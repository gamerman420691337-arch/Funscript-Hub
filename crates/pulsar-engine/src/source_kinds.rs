//! Engine-owned source semantics; filenames never determine eligibility for media decoding.
use super::*;

pub(super) fn parse(value: &str) -> PResult<SourceKind> {
    serde_json::from_value(serde_json::Value::String(value.to_string())).map_err(internal)
}
pub(super) fn kind(
    db: &Connection,
    project: &ProjectId,
    source: &SourceVersionId,
) -> PResult<SourceKind> {
    let raw: Option<String> = db
        .query_row(
            "SELECT kind FROM sources WHERE project=?1 AND version=?2",
            params![project.as_str(), source.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    parse(&raw.ok_or_else(|| {
        ProtocolError::new(ErrorCode::NotFound, "source not imported into this project")
    })?)
}
pub(super) fn require_media(
    db: &Connection,
    project: &ProjectId,
    source: &SourceVersionId,
) -> PResult<()> {
    if kind(db, project, source)? != SourceKind::Media {
        return Err(ProtocolError::invalid("selected source is not media; generation inputs and funscripts cannot be decoded as video"));
    }
    Ok(())
}
pub(super) fn initialize(db: &mut Connection) -> Result<()> {
    let has_kind = {
        let mut statement = db.prepare("PRAGMA table_info(sources)")?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        names.iter().any(|name| name == "kind")
    };
    let tx = db.transaction()?;
    if !has_kind {
        tx.execute(
            "ALTER TABLE sources ADD COLUMN kind TEXT NOT NULL DEFAULT 'media'",
            [],
        )?;
    }
    let jobs = {
        let mut statement = tx.prepare("SELECT project,source,manifest FROM jobs")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (project, source, raw) in jobs {
        let manifest: WorkerRequest = decode(&raw)?;
        anyhow::ensure!(
            manifest.project_id.as_str() == project
                && manifest.source.source_version.as_str() == source,
            "persisted job source association is inconsistent"
        );
        if matches!(
            &manifest.operation,
            WorkerOperation::GenerateInput {
                input: GenerationInput::Text { .. } | GenerationInput::Patterns { .. },
            }
        ) {
            tx.execute(
                "UPDATE sources SET kind='generation_input' WHERE project=?1 AND version=?2",
                params![project, source],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}
