//! Native Stash Media Server GraphQL client for discovering and linking interactive funscripts.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub auto_rescan: bool,
}

impl Default for StashConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:9999/graphql".to_string(),
            api_key: None,
            auto_rescan: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StashFile {
    pub path: String,
    pub duration: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StashScene {
    pub id: String,
    pub title: Option<String>,
    pub details: Option<String>,
    pub files: Vec<StashFile>,
    pub interactive: bool,
}

pub struct StashClient {
    pub config: StashConfig,
}

impl StashClient {
    pub fn new(config: StashConfig) -> Self {
        Self { config }
    }

    /// Helper to execute a raw GraphQL query
    pub fn execute_graphql(&self, query: &str, variables: serde_json::Value) -> Result<serde_json::Value> {
        let body = json!({
            "query": query,
            "variables": variables,
        });

        let config = ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .build();
        let agent: ureq::Agent = config.into();
        let mut req = agent.post(&self.config.endpoint)
            .header("Content-Type", "application/json");

        if let Some(ref key) = self.config.api_key {
            if !key.trim().is_empty() {
                req = req.header("ApiKey", key.trim());
            }
        }

        let payload = serde_json::to_vec(&body)
            .context("Failed to serialize GraphQL body")?;

        let resp = req
            .send(payload)
            .context("Failed to communicate with Stash GraphQL server")?;

        let text = resp
            .into_body()
            .read_to_string()
            .context("Failed to read Stash GraphQL response")?;

        let resp_json: serde_json::Value = serde_json::from_str(&text)
            .context("Failed to parse Stash GraphQL JSON response")?;

        if let Some(errors) = resp_json.get("errors") {
            anyhow::bail!("Stash GraphQL returned errors: {}", errors);
        }

        Ok(resp_json.get("data").cloned().unwrap_or(serde_json::Value::Null))
    }

    /// Test connectivity and query Stash server version
    pub fn test_connection(&self) -> Result<String> {
        let query = r#"
            query GetVersion {
                version {
                    version
                }
            }
        "#;
        let data = self.execute_graphql(query, json!({}))?;
        let ver = data
            .get("version")
            .and_then(|v| v.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        Ok(ver.to_string())
    }

    /// Query Stash for scenes that do NOT have an interactive funscript attached
    pub fn find_scenes_missing_scripts(&self, limit: u32) -> Result<Vec<StashScene>> {
        let query = r#"
            query FindScenesMissingScripts($filter: FindFilterType, $scene_filter: SceneFilterType) {
                findScenes(filter: $filter, scene_filter: $scene_filter) {
                    count
                    scenes {
                        id
                        title
                        details
                        interactive
                        files {
                            path
                            duration
                            width
                            height
                        }
                    }
                }
            }
        "#;

        let variables = json!({
            "filter": {
                "per_page": limit,
                "sort": "title",
                "direction": "ASC"
            },
            "scene_filter": {
                "interactive": {
                    "value": false,
                    "modifier": "EQUALS"
                }
            }
        });

        let data = self.execute_graphql(query, variables)?;
        let scenes_val = data
            .get("findScenes")
            .and_then(|f| f.get("scenes"))
            .cloned()
            .unwrap_or_else(|| json!([]));

        let scenes: Vec<StashScene> = serde_json::from_value(scenes_val)
            .context("Failed to deserialize StashScene records")?;

        Ok(scenes)
    }

    /// Request Stash to trigger a metadata scan on specific file paths to detect new funscripts
    pub fn trigger_metadata_scan(&self, paths: &[String]) -> Result<bool> {
        let query = r#"
            mutation MetadataScan($input: ScanMetadataInput!) {
                metadataScan(input: $input)
            }
        "#;

        let variables = json!({
            "input": {
                "paths": paths
            }
        });

        let data = self.execute_graphql(query, variables)?;
        let ok = data.get("metadataScan").and_then(|v| v.as_bool()).unwrap_or(true);
        Ok(ok)
    }

    /// Create or find a tag and attach it to a scene in Stash
    pub fn tag_scene(&self, scene_id: &str, tag_name: &str) -> Result<()> {
        let find_tag_query = r#"
            query FindTag($name: String!) {
                findTags(tag_filter: { name: { value: $name, modifier: EQUALS } }) {
                    tags { id name }
                }
            }
        "#;
        let data = self.execute_graphql(find_tag_query, json!({ "name": tag_name }))?;
        let tag_id = if let Some(tags) = data.get("findTags").and_then(|f| f.get("tags")).and_then(|t| t.as_array()) {
            if let Some(first) = tags.first() {
                first.get("id").and_then(|i| i.as_str()).map(|s| s.to_string())
            } else {
                None
            }
        } else {
            None
        };

        let effective_tag_id = match tag_id {
            Some(id) => id,
            None => {
                let create_tag_query = r#"
                    mutation TagCreate($input: TagCreateInput!) {
                        tagCreate(input: $input) { id }
                    }
                "#;
                let cdata = self.execute_graphql(create_tag_query, json!({ "input": { "name": tag_name } }))?;
                cdata.get("tagCreate").and_then(|t| t.get("id")).and_then(|i| i.as_str()).unwrap_or("").to_string()
            }
        };

        if !effective_tag_id.is_empty() {
            let update_query = r#"
                mutation SceneUpdate($input: SceneUpdateInput!) {
                    sceneUpdate(input: $input) { id }
                }
            "#;
            let _ = self.execute_graphql(update_query, json!({
                "input": {
                    "id": scene_id,
                    "tag_ids": [effective_tag_id]
                }
            }))?;
        }

        Ok(())
    }
}

/// Helper function to build GraphQL query payloads for offline testing
#[allow(dead_code)]
pub fn build_missing_scenes_query(limit: u32) -> serde_json::Value {
    json!({
        "query": "query FindScenesMissingScripts($filter: FindFilterType, $scene_filter: SceneFilterType) { findScenes(filter: $filter, scene_filter: $scene_filter) { count scenes { id title details interactive files { path duration width height } } } }",
        "variables": {
            "filter": { "per_page": limit, "sort": "title", "direction": "ASC" },
            "scene_filter": { "interactive": { "value": false, "modifier": "EQUALS" } }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stash_query_serialization() {
        let payload = build_missing_scenes_query(25);
        assert!(payload["query"].as_str().unwrap().contains("findScenes"));
        assert_eq!(payload["variables"]["filter"]["per_page"], 25);
        assert_eq!(payload["variables"]["scene_filter"]["interactive"]["value"], false);
    }

    #[test]
    fn test_stash_scene_deserialization() {
        let json_data = json!([
            {
                "id": "1042",
                "title": "Stash Test Scene",
                "details": "Demo description",
                "interactive": false,
                "files": [
                    {
                        "path": "/videos/test_scene.mp4",
                        "duration": 480.5,
                        "width": 1920,
                        "height": 1080
                    }
                ]
            }
        ]);

        let scenes: Vec<StashScene> = serde_json::from_value(json_data).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].id, "1042");
        assert_eq!(scenes[0].title.as_deref(), Some("Stash Test Scene"));
        assert_eq!(scenes[0].files.len(), 1);
        assert_eq!(scenes[0].files[0].path, "/videos/test_scene.mp4");
        assert_eq!(scenes[0].files[0].width, Some(1920));
        assert!(!scenes[0].interactive);
    }
}
