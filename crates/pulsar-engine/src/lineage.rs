//! Durable revision ancestry and unresolved review snapshots. No equality-based origin inference.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ReviewState {
    pub(super) flags: Vec<ReviewFlag>,
    pub(super) unknown: bool,
    pub(super) conservative: bool,
}
impl ReviewState {
    fn empty() -> Self {
        Self {
            flags: vec![],
            unknown: false,
            conservative: false,
        }
    }
}

pub(super) fn initialize(db: &mut Connection) -> Result<()> {
    let tx = db.transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS revision_lineage(
            project TEXT NOT NULL, revision INTEGER NOT NULL,
            parent_revision INTEGER, restored_from_revision INTEGER, restored_history_position INTEGER,
            candidate TEXT, operation TEXT NOT NULL, contributions TEXT NOT NULL, review_state TEXT NOT NULL,
            PRIMARY KEY(project,revision), FOREIGN KEY(project,revision) REFERENCES revisions(project,revision),
            FOREIGN KEY(project,parent_revision) REFERENCES revisions(project,revision),
            FOREIGN KEY(project,restored_from_revision) REFERENCES revisions(project,revision),
            FOREIGN KEY(candidate) REFERENCES candidates(id));
         CREATE TABLE IF NOT EXISTS history_lineage(
            project TEXT NOT NULL, position INTEGER NOT NULL, revision INTEGER NOT NULL,
            PRIMARY KEY(project,position),
            FOREIGN KEY(project,position) REFERENCES edit_states(project,position) ON DELETE CASCADE,
            FOREIGN KEY(project,revision) REFERENCES revisions(project,revision));"
    )?;
    let projects = {
        let mut statement = tx.prepare("SELECT id,revision,history_cursor FROM projects WHERE NOT EXISTS(SELECT 1 FROM project_import_archives WHERE project_import_archives.project=projects.id) AND NOT EXISTS(
            SELECT 1 FROM revision_lineage WHERE revision_lineage.project=projects.id AND revision_lineage.revision=projects.revision)")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (project, revision, cursor) in projects {
        // Only an existing explicit candidate->committed revision link is reused.
        // We never guess an older undo/redo ancestry from equal program bytes.
        let reviews = {
            let mut statement = tx.prepare(
                "SELECT review FROM candidates WHERE project=?1 AND committed_revision=?2",
            )?;
            let rows =
                statement.query_map(params![project, revision], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut state = ReviewState {
            flags: vec![],
            unknown: true,
            conservative: true,
        };
        for raw in reviews {
            state.flags.extend(decode::<Vec<ReviewFlag>>(&raw)?);
        }
        validate_reviews(&state.flags)?;
        anyhow::ensure!(
            encode(&state)?.len() <= PROGRAM_LIMIT,
            "legacy review snapshot exceeds 512 KiB"
        );
        tx.execute("INSERT INTO revision_lineage VALUES(?1,?2,NULL,NULL,NULL,NULL,'legacy_unknown','[]',?3)",
            params![project,revision,encode(&state)?])?;
        tx.execute(
            "INSERT OR IGNORE INTO history_lineage VALUES(?1,?2,?3)",
            params![project, cursor, revision],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn state(
    db: &Connection,
    project: &ProjectId,
    revision: RevisionId,
) -> PResult<ReviewState> {
    let raw: Option<String> = db
        .query_row(
            "SELECT review_state FROM revision_lineage WHERE project=?1 AND revision=?2",
            params![project.as_str(), rev_sql(revision)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    let mut state: ReviewState = match raw {
        Some(raw) => decode(&raw)?,
        None => ReviewState {
            flags: vec![],
            unknown: true,
            conservative: true,
        },
    };
    if project_package_imports::imported_revision(db, project, revision)? { state.unknown=true; state.conservative=true; }
    validate_reviews(&state.flags)?;
    Ok(state)
}

pub(super) fn created(tx: &Transaction<'_>, project: &ProjectId) -> PResult<()> {
    tx.execute(
        "INSERT INTO revision_lineage VALUES(?1,0,NULL,NULL,NULL,NULL,'create','[]',?2)",
        params![project.as_str(), encode(&ReviewState::empty())?],
    )
    .map_err(internal)?;
    tx.execute(
        "INSERT INTO history_lineage VALUES(?1,0,0)",
        [project.as_str()],
    )
    .map_err(internal)?;
    Ok(())
}

pub(super) fn coverage(program: &MotionProgram) -> PResult<Vec<(Axis, TimeRange)>> {
    let mut output = Vec::new();
    for track in program.tracks() {
        let mut spans: Vec<_> = track
            .span()
            .map_err(internal)?
            .into_iter()
            .chain(track.gaps().iter().copied())
            .collect();
        spans.sort_by_key(|span| span.start());
        let mut merged: Vec<TimeRange> = Vec::new();
        for span in spans {
            if let Some(last) = merged.last_mut() {
                if span.start() <= last.end() {
                    *last = TimeRange::new(last.start(), last.end().max(span.end()))
                        .map_err(internal)?;
                    continue;
                }
            }
            merged.push(span);
        }
        output.extend(merged.into_iter().map(|span| (track.axis(), span)));
    }
    Ok(output)
}

pub(super) fn clipped(
    flags: &[ReviewFlag],
    regions: &[(Axis, TimeRange)],
) -> PResult<Vec<ReviewFlag>> {
    let mut output = Vec::new();
    for flag in flags {
        for (axis, region) in regions {
            if flag.axis.is_some_and(|selected| selected != *axis) || !flag.range.overlaps(*region)
            {
                continue;
            }
            if output.len() >= 1024 {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "review contribution snapshot exceeds 1024 flags",
                ));
            }
            output.push(ReviewFlag {
                axis: Some(*axis),
                range: TimeRange::new(
                    flag.range.start().max(region.start()),
                    flag.range.end().min(region.end()),
                )
                .map_err(internal)?,
                reason: flag.reason.clone(),
                evidence: flag.evidence,
                confidence: flag.confidence.clone(),
            });
        }
    }
    validate_reviews(&output)?;
    Ok(output)
}

/// Called inside the same transaction as the revision and history transition.
pub(super) fn record(
    tx: &Transaction<'_>,
    request: &Request,
    revision: RevisionId,
    parent: RevisionId,
    before: &MotionProgram,
    program: &MotionProgram,
    append: bool,
) -> PResult<()> {
    let project = project(request)?;
    let prior = state(tx, project, parent)?;
    let mut next = prior.clone();
    let mut candidate_id = None;
    let mut restored_from = None;
    let mut restored_position = None;
    let mut contributions = Vec::new();
    let operation = match &request.command {
        Command::CommitCandidate { candidate_id: id } => {
            let candidate = candidate_state(tx, id, project)?;
            next = candidate_review_state(tx, id)?.unwrap_or(ReviewState {
                flags: candidate.review,
                unknown: false,
                conservative: false,
            });
            candidate_id = Some(id.as_str());
            contributions = coverage(program)?;
            "candidate_commit"
        }
        Command::MergeCandidate {
            candidate_id: id,
            axes,
            range,
        } => {
            let candidate = candidate_state(tx, id, project)?;
            let raw: String = tx
                .query_row(
                    "SELECT protected FROM projects WHERE id=?1",
                    [project.as_str()],
                    |row| row.get(0),
                )
                .map_err(internal)?;
            let protected: Vec<ProtectedRegion> = decode(&raw)?;
            contributions = pulsar_core::candidate_merge_writable_regions(
                before,
                &candidate.program,
                axes,
                *range,
                &protected,
            )
            .map_err(|error| ProtocolError::invalid(error.to_string()))?;
            next.flags = clipped(&prior.flags, &coverage(program)?)?;
            next.flags
                .extend(clipped(&candidate.review, &contributions)?);
            // Previous flags may now cover unchanged or replaced samples; they
            // remain visibly conservative until explicit resolution exists.
            next.conservative = true;
            next.unknown |= candidate_review_state(tx, id)?.is_some_and(|state| state.unknown);
            candidate_id = Some(id.as_str());
            "candidate_merge"
        }
        Command::Undo | Command::Redo => {
            let cursor: i64 = tx
                .query_row(
                    "SELECT history_cursor FROM projects WHERE id=?1",
                    [project.as_str()],
                    |row| row.get(0),
                )
                .map_err(internal)?;
            let delta = if matches!(&request.command, Command::Undo) {
                -1
            } else {
                1
            };
            let target = cursor
                .checked_add(delta)
                .ok_or_else(|| ProtocolError::invalid("history lineage position overflow"))?;
            restored_position = Some(target);
            let source: Option<i64> = tx
                .query_row(
                    "SELECT revision FROM history_lineage WHERE project=?1 AND position=?2",
                    params![project.as_str(), target],
                    |row| row.get(0),
                )
                .optional()
                .map_err(internal)?;
            if let Some(source) = source {
                restored_from = Some(source);
                next = state(
                    tx,
                    project,
                    RevisionId::new(u64::try_from(source).map_err(internal)?),
                )?;
            } else {
                next.flags = clipped(&prior.flags, &coverage(program)?)?;
                next.unknown = true;
                next.conservative = true;
            }
            if delta < 0 {
                "undo"
            } else {
                "redo"
            }
        }
        Command::SetProtectedRegions { .. } => "protection",
        _ => {
            next.flags = clipped(&prior.flags, &coverage(program)?)?;
            next.conservative |= !next.flags.is_empty();
            "edit"
        }
    };
    validate_reviews(&next.flags)
        .map_err(|error| ProtocolError::new(ErrorCode::ResourceExhausted, error.to_string()))?;
    let encoded = encode(&next)?;
    if encoded.len() > PROGRAM_LIMIT {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "revision review snapshot exceeds 512 KiB",
        ));
    }
    tx.execute(
        "INSERT INTO revision_lineage VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            project.as_str(),
            rev_sql(revision)?,
            rev_sql(parent)?,
            restored_from,
            restored_position,
            candidate_id,
            operation,
            encode(&contributions)?,
            encoded
        ],
    )
    .map_err(internal)?;
    if append {
        let position: i64 = tx
            .query_row(
                "SELECT history_cursor FROM projects WHERE id=?1",
                [project.as_str()],
                |row| row.get(0),
            )
            .map_err(internal)?;
        tx.execute(
            "INSERT INTO history_lineage VALUES(?1,?2,?3)",
            params![project.as_str(), position, rev_sql(revision)?],
        )
        .map_err(internal)?;
    }
    Ok(())
}

pub(super) fn candidate_review_state(
    db: &Connection,
    candidate: &CandidateId,
) -> PResult<Option<ReviewState>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT review_state FROM edit_candidate_lineage WHERE candidate=?1",
            [candidate.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    let state: Option<ReviewState> = raw.map(|raw| decode(&raw)).transpose()?;
    if let Some(state) = &state {
        validate_reviews(&state.flags)?;
    }
    if state.is_none() { return project_package_imports::candidate_review(db, candidate); }
    Ok(state)
}
