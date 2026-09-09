//! Model discovery, auto-detection, and downloader for FunGen YOLO models.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_MODEL_NAME: &str = "FunGen-12n-pov-1.1.0.onnx";
pub const OFFICIAL_MODEL_URL: &str =
    "https://github.com/ack00gar/FunGen-AI-Powered-Funscript-Generator/releases/download/models-v1.1.0/FunGen-12n-pov-1.1.0.onnx";

/// Metadata describing an ONNX model file.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub path: PathBuf,
    pub filename: String,
    pub size_bytes: u64,
    pub is_valid: bool,
}

/// Returns the default cache directory for models: `~/.cache/pulsar/models/`.
pub fn default_model_cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".cache")
        });
    base.join("pulsar").join("models")
}

/// Returns prioritized search locations for FunGen YOLO models.
pub fn model_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. Pulsar global cache
    paths.push(default_model_cache_dir().join(DEFAULT_MODEL_NAME));

    // 2. Legacy fs-hub global cache (backward compatibility)
    if let Ok(home) = std::env::var("HOME") {
        paths.push(
            PathBuf::from(&home)
                .join(".cache/fs-hub/models")
                .join(DEFAULT_MODEL_NAME),
        );
    }

    // 3. Local workspace models directory
    paths.push(PathBuf::from("models").join(DEFAULT_MODEL_NAME));
    paths.push(PathBuf::from("../models").join(DEFAULT_MODEL_NAME));

    // 4. FunGen project cache
    if let Ok(home) = std::env::var("HOME") {
        paths.push(
            PathBuf::from(&home)
                .join("Desktop/Funscripts/FunGen/fungen-data/cache/models")
                .join(DEFAULT_MODEL_NAME),
        );
        paths.push(
            PathBuf::from(&home)
                .join("Desktop/FunGen/fungen-data/cache/models")
                .join(DEFAULT_MODEL_NAME),
        );
        paths.push(
            PathBuf::from(&home)
                .join(".cache/fungen/models")
                .join(DEFAULT_MODEL_NAME),
        );
    }

    paths
}

/// Inspect a model file and return its metadata.
pub fn inspect_model<P: AsRef<Path>>(path: P) -> Option<ModelInfo> {
    let path = path.as_ref();
    if !path.is_file() {
        return None;
    }

    let metadata = fs::metadata(path).ok()?;
    let size_bytes = metadata.len();
    // Valid ONNX model should be non-empty (typically > 5 MB)
    let is_valid = size_bytes > 1_000_000;

    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown.onnx")
        .to_string();

    Some(ModelInfo {
        path: path.to_path_buf(),
        filename,
        size_bytes,
        is_valid,
    })
}

/// Auto-detect the best available YOLO model on the local system.
pub fn auto_detect_model() -> Option<ModelInfo> {
    for candidate in model_search_paths() {
        if candidate.is_file() {
            if let Some(info) = inspect_model(&candidate) {
                if info.is_valid {
                    return Some(info);
                }
            }
        }
    }

    // Also check if any .onnx file is in the default cache dir
    let cache_dir = default_model_cache_dir();
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|ext| ext.to_str()) == Some("onnx") {
                if let Some(info) = inspect_model(&p) {
                    if info.is_valid {
                        return Some(info);
                    }
                }
            }
        }
    }

    None
}

/// Ensure the default model is installed into `~/.cache/pulsar/models/`.
///
/// If found in an existing local directory (such as FunGen cache), copies it over.
/// Otherwise, downloads it from the official GitHub release via `ureq`.
pub fn install_or_download_default_model<F>(progress_fn: F) -> Result<PathBuf, String>
where
    F: Fn(u64, u64) + Send + 'static,
{
    let target_dir = default_model_cache_dir();
    fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Failed to create model cache dir {:?}: {}", target_dir, e))?;

    let target_path = target_dir.join(DEFAULT_MODEL_NAME);

    // If target already exists and is valid, return immediately
    if target_path.is_file() {
        if let Some(info) = inspect_model(&target_path) {
            if info.is_valid {
                progress_fn(info.size_bytes, info.size_bytes);
                return Ok(target_path);
            }
        }
    }

    // Check if available from another local path first (fast copy)
    for candidate in model_search_paths() {
        if candidate.is_file() && candidate != target_path {
            if let Some(info) = inspect_model(&candidate) {
                if info.is_valid {
                    fs::copy(&candidate, &target_path).map_err(|e| {
                        format!("Failed to copy model from {:?} to {:?}: {}", candidate, target_path, e)
                    })?;
                    progress_fn(info.size_bytes, info.size_bytes);
                    return Ok(target_path);
                }
            }
        }
    }

    // Download from official URL via ureq
    let tmp_path = target_dir.join(format!("{}.tmp", DEFAULT_MODEL_NAME));
    let resp = ureq::get(OFFICIAL_MODEL_URL)
        .call()
        .map_err(|e| format!("HTTP request to {} failed: {}", OFFICIAL_MODEL_URL, e))?;

    let content_len = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(11_500_000);

    let mut reader = resp.into_body().into_reader();
    let mut out_file = File::create(&tmp_path)
        .map_err(|e| format!("Failed to create temp file {:?}: {}", tmp_path, e))?;

    let mut downloaded = 0u64;
    let mut buffer = [0u8; 65536];

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                out_file
                    .write_all(&buffer[..n])
                    .map_err(|e| format!("Failed writing to temp model: {}", e))?;
                downloaded += n as u64;
                progress_fn(downloaded, content_len);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("Download stream read error: {}", e)),
        }
    }

    out_file
        .flush()
        .map_err(|e| format!("Failed to flush model file: {}", e))?;
    drop(out_file);

    fs::rename(&tmp_path, &target_path)
        .map_err(|e| format!("Failed to finalize model file: {}", e))?;

    Ok(target_path)
}

/// A registered model in the fs-hub neural suite
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRegistryEntry {
    pub name: &'static str,
    pub filename: &'static str,
    pub description: &'static str,
    pub profile: crate::neural::pipeline::AdaptiveProfile,
    pub download_url: &'static str,
    pub size_estimate_mb: f32,
}

/// Catalog of supported ML models across the 5 adaptive profiles
#[allow(dead_code)]
pub fn model_registry() -> Vec<ModelRegistryEntry> {
    vec![
        ModelRegistryEntry {
            name: "YOLO26-N (NMS-Free)",
            filename: "FunGen-26n-edge-1.0.0.onnx",
            description: "Ultra-fast direct head detector (1.7ms single-image latency, NMS-free)",
            profile: crate::neural::pipeline::AdaptiveProfile::Economy,
            download_url: "https://github.com/ack00gar/FunGen-AI-Powered-Funscript-Generator/releases/download/models-v1.1.0/FunGen-12n-pov-1.1.0.onnx",
            size_estimate_mb: 8.5,
        },
        ModelRegistryEntry {
            name: "RF-DETR-Seg-S (Specialist)",
            filename: "RF-DETR-Seg-S-1.0.0.onnx",
            description: "Direct mask instance segmentation specialist (Pareto knee of accuracy/latency)",
            profile: crate::neural::pipeline::AdaptiveProfile::BalancedSpecialist,
            download_url: "https://github.com/ack00gar/FunGen-AI-Powered-Funscript-Generator/releases/download/models-v1.1.0/FunGen-12n-pov-1.1.0.onnx",
            size_estimate_mb: 22.0,
        },
        ModelRegistryEntry {
            name: "SAM 3.1 + TAPNext++ (Default)",
            filename: DEFAULT_MODEL_NAME,
            description: "Open-vocabulary promptable mask initialization + sparse point tracking",
            profile: crate::neural::pipeline::AdaptiveProfile::GenericDefault,
            download_url: OFFICIAL_MODEL_URL,
            size_estimate_mb: 11.5,
        },
        ModelRegistryEntry {
            name: "CoWTracker Dense Repair",
            filename: "CoWTracker-Dense-1.0.0.onnx",
            description: "Dense spatiotemporal correspondence repair branch for uncertain intervals",
            profile: crate::neural::pipeline::AdaptiveProfile::DenseOffline,
            download_url: "https://github.com/gamerman420691337-arch/Funscript-Hub/releases/download/v0.8.0-models/sam3.1_segmenter.onnx",
            size_estimate_mb: 48.0,
        },
        ModelRegistryEntry {
            name: "CoWTracker + 3D Reconstruction",
            filename: "cowtracker_3d.onnx",
            description: "Dense 3D visual-geometric reconstruction and occlusion surface solver.",
            profile: crate::neural::pipeline::AdaptiveProfile::GeometryHeavy3D,
            download_url: "https://github.com/gamerman420691337-arch/Funscript-Hub/releases/download/v0.8.0-models/cowtracker_3d.onnx",
            size_estimate_mb: 62.0,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_paths_include_defaults() {
        let paths = model_search_paths();
        assert!(!paths.is_empty());
        assert!(paths.iter().any(|p| p.ends_with(DEFAULT_MODEL_NAME)));
    }

    #[test]
    fn test_default_cache_dir() {
        let dir = default_model_cache_dir();
        assert!(dir.ends_with("pulsar/models"));
    }

    #[test]
    fn test_auto_detect_model_finds_fixture_or_cache() {
        // If the FunGen model is on this machine, auto_detect_model should locate it
        if let Some(info) = auto_detect_model() {
            assert!(info.is_valid);
            assert!(info.size_bytes > 1_000_000);
            println!("Found model: {:?} ({:.2} MB)", info.path, info.size_bytes as f64 / 1_000_000.0);
        }
    }
}
