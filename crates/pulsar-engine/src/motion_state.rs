//! SQL references to durable immutable motion. Inline legacy bytes remain recoverable.
use super::*;
use crate::motion_artifacts::{ProgramStore, StoredProgram};

pub(super) fn store_for(db: &Connection) -> PResult<ProgramStore> {
    let path = db
        .path()
        .ok_or_else(|| internal("motion storage requires a file-backed database"))?;
    let parent = Path::new(path)
        .parent()
        .ok_or_else(|| internal("motion database has no parent"))?;
    ProgramStore::new(&parent.join("motion-objects"))
}
pub(super) fn register(db: &Connection, descriptor: &StoredProgram) -> PResult<String> {
    descriptor.validate()?;
    db.execute(
        "INSERT OR IGNORE INTO motion_objects(sha256,descriptor) VALUES(?1,?2)",
        params![descriptor.sha256, encode(descriptor)?],
    )
    .map_err(internal)?;
    let recorded: String = db
        .query_row(
            "SELECT descriptor FROM motion_objects WHERE sha256=?1",
            [&descriptor.sha256],
            |row| row.get(0),
        )
        .map_err(internal)?;
    if recorded != encode(descriptor)? {
        return Err(internal(
            "motion object metadata conflicts with immutable identity",
        ));
    }
    Ok(format!("motion:{}", descriptor.sha256))
}
pub(super) fn save(db: &Connection, program: &MotionProgram) -> PResult<String> {
    let descriptor = store_for(db)?.publish(program)?;
    register(db, &descriptor)
}
pub(super) fn descriptor(db: &Connection, reference: &str) -> PResult<StoredProgram> {
    let digest = reference
        .strip_prefix("motion:")
        .ok_or_else(|| internal("unmigrated inline motion reference"))?;
    let raw: Option<String> = db
        .query_row(
            "SELECT descriptor FROM motion_objects WHERE sha256=?1",
            [digest],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    let descriptor: StoredProgram =
        decode(&raw.ok_or_else(|| internal("motion object reference is missing"))?)?;
    descriptor.validate()?;
    if descriptor.sha256 != digest {
        return Err(internal("motion reference identity mismatch"));
    }
    Ok(descriptor)
}
pub(super) fn load(db: &Connection, reference: &str) -> PResult<MotionProgram> {
    store_for(db)?.read(&descriptor(db, reference)?)
}
pub(super) fn revision_descriptor(
    db: &Connection,
    project: &ProjectId,
    revision: RevisionId,
) -> PResult<StoredProgram> {
    let reference: Option<String> = db
        .query_row(
            "SELECT program FROM revisions WHERE project=?1 AND revision=?2",
            params![project.as_str(), rev_sql(revision)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    descriptor(
        db,
        &reference.ok_or_else(|| {
            ProtocolError::new(ErrorCode::NotFound, "revision not found in project")
        })?,
    )
}
pub(super) fn initialize(db: &mut Connection) -> Result<()> {
    let store = store_for(db)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS motion_objects(sha256 TEXT PRIMARY KEY,descriptor TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS legacy_motion_bytes(owner_table TEXT NOT NULL,row_key TEXT NOT NULL,program TEXT NOT NULL,PRIMARY KEY(owner_table,row_key));")?;
    // Keep every original inline cell, including its exact historical JSON bytes.
    // Objects are durable before references commit; failed migration leaves only reclaimable orphans.
    for (table, key) in [
        ("projects", "id"),
        ("revisions", "project||':'||revision"),
        ("edit_states", "project||':'||position"),
        ("candidates", "id"),
    ] {
        let sql=format!("SELECT rowid,{key},program FROM {table} WHERE program NOT LIKE 'motion:%' ORDER BY rowid");
        let mut statement = tx.prepare(&sql)?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let rowid: i64 = row.get(0)?;
            let row_key: String = row.get(1)?;
            let raw: String = row.get(2)?;
            anyhow::ensure!(
                raw.len() <= 64 * 1024 * 1024,
                "legacy motion exceeds bounded migration admission"
            );
            let program: MotionProgram = serde_json::from_str(&raw)?;
            let descriptor = store.publish(&program)?;
            let reference = register(&tx, &descriptor)?;
            tx.execute(
                "INSERT OR IGNORE INTO legacy_motion_bytes VALUES(?1,?2,?3)",
                params![table, row_key, raw],
            )?;
            tx.execute(
                &format!("UPDATE {table} SET program=?1 WHERE rowid=?2"),
                params![reference, rowid],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}
