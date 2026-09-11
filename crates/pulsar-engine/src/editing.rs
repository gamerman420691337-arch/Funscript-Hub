//! Scoped imports, selective candidate commits and read-only review diagnostics.
use super::*;
use std::io::Read;

pub(super) struct PreparedFunscript {
    pub(super) program: ProgramDescriptor,
    pub(super) identity: ArtifactIdentity,
    pub(super) path: PathBuf,
    pub(super) label: String,
    pub(super) review: Vec<ReviewFlag>,
}

#[derive(Deserialize)]
struct FunscriptInput {
    actions: Vec<FunscriptAction>,
    #[serde(default)]
    inverted: bool,
}
#[derive(Deserialize)]
struct FunscriptAction {
    at: u64,
    pos: u8,
}

impl Engine {
    pub(super) fn prepare_funscript(&self, source: &Path) -> PResult<PreparedFunscript> {
        let (path, hash, bytes) = artifacts::snapshot_reserved(
            source,
            &self.config.state_dir.join("snapshots"),
            MAX_MOTION_BYTES.min(self.config.max_snapshot_bytes),
            self.pool.reserved_storage(),
        )
        .map_err(|error| ProtocolError::invalid(error.to_string()))?;
        let input: FunscriptInput = serde_json::from_reader(
            artifacts::open_regular(&path)
                .map_err(unavailable)?
                .take(MAX_MOTION_BYTES + 1),
        )
        .map_err(|error| ProtocolError::invalid(format!("invalid funscript: {error}")))?;
        if input.actions.is_empty() {
            return Err(ProtocolError::invalid("funscript contains no actions"));
        }
        let mut actions = Vec::with_capacity(input.actions.len());
        for action in input.actions {
            if action.pos > 100 {
                return Err(ProtocolError::invalid("funscript position exceeds 100"));
            }
            let time = action
                .at
                .checked_mul(1_000_000)
                .and_then(|time| i64::try_from(time).ok())
                .ok_or_else(|| ProtocolError::invalid("funscript timestamp overflow"))?;
            let position = f64::from(if input.inverted {
                100 - action.pos
            } else {
                action.pos
            }) / 100.0;
            actions.push(
                MotionAction::new(
                    ProjectTime::from_nanos(time),
                    NormalizedPosition::new(position)
                        .map_err(|error| ProtocolError::invalid(error.to_string()))?,
                    EvidenceKind::Synthesized,
                )
                .map_err(|error| ProtocolError::invalid(error.to_string()))?,
            );
        }
        let track = MotionTrack::new(Axis::Stroke, actions)
            .map_err(|error| ProtocolError::invalid(error.to_string()))?;
        let range = track
            .span()
            .map_err(|error| ProtocolError::invalid(error.to_string()))?
            .ok_or_else(|| ProtocolError::invalid("funscript has no time span"))?;
        let program = MotionProgram::new(vec![track]).map_err(internal)?;
        validate_program(&program)?;
        let review = vec![ReviewFlag {
            axis: Some(Axis::Stroke),
            range,
            reason: ReviewReason::new("imported_motion")?,
            evidence: EvidenceKind::Synthesized,
            confidence: None,
        }];
        Ok(PreparedFunscript {
            program: self.motion_store.publish(&program)?,
            review,
            path,
            label: source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            identity: ArtifactIdentity {
                artifact_id: ArtifactId::new(hash.clone()).map_err(internal)?,
                sha256: hash,
                byte_len: bytes,
            },
        })
    }

    pub(super) fn materialize_input(&self, input: &GenerationInput) -> PResult<SourceArtifact> {
        let bytes = serde_json::to_vec(input).map_err(internal)?;
        if bytes.len() > PROGRAM_LIMIT {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "generation input exceeds bounded control snapshot",
            ));
        }
        let mut temporary =
            tempfile::NamedTempFile::new_in(self.config.state_dir.join("snapshots"))
                .map_err(unavailable)?;
        temporary.write_all(&bytes).map_err(unavailable)?;
        temporary.as_file().sync_all().map_err(unavailable)?;
        self.snapshot_model(temporary.path())
    }

    pub(super) fn import_funscript(
        &self,
        tx: &Transaction<'_>,
        request: &Request,
        prepared: PreparedFunscript,
    ) -> PResult<ResponseBody> {
        let snapshot = project_snapshot(tx, project(request)?)?;
        let candidate = id!(CandidateId);
        let registered: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sources WHERE project=?1 AND version=?2)",
                params![snapshot.project_id.as_str(), prepared.identity.sha256],
                |row| row.get(0),
            )
            .map_err(internal)?;
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM sources WHERE project=?1",
                [snapshot.project_id.as_str()],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if !registered && count >= 128 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "project source catalog is full",
            ));
        }
        tx.execute(
            "INSERT INTO sources(project,version,path,original,label,sha256,bytes,kind)
            VALUES(?1,?2,?3,'',?4,?2,?5,'funscript') ON CONFLICT(project,version)
            DO UPDATE SET evicted=0,path=excluded.path,kind=excluded.kind",
            params![
                snapshot.project_id.as_str(),
                prepared.identity.sha256,
                prepared.path.to_string_lossy(),
                prepared.label,
                i64::try_from(prepared.identity.byte_len).map_err(internal)?
            ],
        )
        .map_err(internal)?;
        tx.execute("INSERT INTO candidates(id,project,base_revision,program,job,lineage,review) VALUES(?1,?2,?3,?4,NULL,?5,?6)",
            params![candidate.as_str(), snapshot.project_id.as_str(), rev_sql(snapshot.revision)?,
                motion_state::register(tx,&prepared.program)?, encode(&vec![prepared.identity])?, encode(&prepared.review)?])
            .map_err(internal)?;
        event(
            tx,
            &snapshot.project_id,
            snapshot.revision,
            EventBody::CandidateReady {
                candidate_id: candidate.clone(),
            },
        )?;
        Ok(ResponseBody::Candidate(candidate_snapshot(
            tx,
            &candidate,
            &snapshot.project_id,
        )?))
    }

    pub(super) fn merge_candidate(
        &self,
        tx: &Transaction<'_>,
        request: &Request,
        session: &SessionId,
        candidate_id: &CandidateId,
        axes: &[Axis],
        range: Option<TimeRange>,
    ) -> PResult<ResponseBody> {
        let candidate = candidate_state(tx, candidate_id, project(request)?)?;
        let snapshot = project_state(tx, project(request)?)?;
        if candidate.base_revision != snapshot.revision {
            return Err(ProtocolError::new(
                ErrorCode::RevisionConflict,
                "candidate is stale; explicitly rebase before merging",
            ));
        }
        let committed: Option<i64> = tx
            .query_row(
                "SELECT committed_revision FROM candidates WHERE id=?1",
                [candidate_id.as_str()],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if committed.is_some() {
            return Err(ProtocolError::new(
                ErrorCode::RequestConflict,
                "candidate already committed",
            ));
        }
        let protected = protected_regions(tx, &snapshot.project_id)?;
        let merged = pulsar_core::merge_candidate_preserving_protection(
            &snapshot.program,
            &candidate.program,
            axes,
            range,
            &protected,
        )
        .map_err(|error| ProtocolError::invalid(error.to_string()))?;
        if merged == snapshot.program {
            return Err(ProtocolError::new(
                ErrorCode::Forbidden,
                "selection contains no writable change; protected segments were preserved",
            ));
        }
        let revision = commit_program(
            tx,
            request,
            session,
            &merged,
            "candidate_merge",
            "Merge selected candidate motion",
            true,
        )?;
        tx.execute(
            "UPDATE candidates SET committed_revision=?2 WHERE id=?1",
            params![candidate_id.as_str(), rev_sql(revision)?],
        )
        .map_err(internal)?;
        Ok(ResponseBody::Project(project_snapshot(
            tx,
            &snapshot.project_id,
        )?))
    }

    pub(super) fn diagnostics(
        &self,
        tx: &Transaction<'_>,
        request: &Request,
        candidate_id: Option<&CandidateId>,
    ) -> PResult<ResponseBody> {
        let snapshot = project_state(tx, project(request)?)?;
        let candidate = candidate_id
            .map(|candidate| candidate_state(tx, candidate, &snapshot.project_id))
            .transpose()?;
        let program = candidate
            .as_ref()
            .map(|candidate| &candidate.program)
            .unwrap_or(&snapshot.program);
        let review_state = if let Some(id) = candidate_id {
            lineage::candidate_review_state(tx, id)?
        } else {
            Some(lineage::state(tx, &snapshot.project_id, snapshot.revision)?)
        };
        let protected = protected_regions(tx, &snapshot.project_id)?;
        let mut issues = Vec::new();
        if review_state.as_ref().is_some_and(|state| state.unknown) {
            issues.push(DiagnosticIssue { code: "review_history_unknown".into(),
                message: "Legacy review ancestry is explicitly unknown; missing links are not inferred and no human resolution is claimed.".into(),
                axis: None, range: None });
        }
        if let Some(id) = candidate_id {
            let receipt:Option<(String,i64,i64)>=tx.query_row("SELECT receipt,authored_count,inherited_count FROM edit_candidate_lineage WHERE candidate=?1",
                [id.as_str()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(internal)?;
            if let Some((raw, authored, inherited)) = receipt {
                let identity: ArtifactIdentity = decode(&raw)?;
                issues.push(DiagnosticIssue{code:"immutable_edit_receipt".into(),message:format!("Exact edit lineage retained in {} ({} authored intervals, {} inherited intervals); displayed reviews are conservative envelopes.",identity.artifact_id,authored,inherited),axis:None,range:None});
            }
        }
        if candidate
            .as_ref()
            .is_some_and(|candidate| candidate.base_revision != snapshot.revision)
        {
            issues.push(DiagnosticIssue {
                code: "candidate_stale".into(),
                message: "Candidate belongs to an earlier revision; explicit rebase is required."
                    .into(),
                axis: None,
                range: None,
            });
        }
        if program
            .tracks()
            .iter()
            .all(|track| track.actions().is_empty())
        {
            issues.push(DiagnosticIssue {
                code: "empty_program".into(),
                message: "No motion actions are available.".into(),
                axis: None,
                range: None,
            });
        }
        for track in program.tracks() {
            for gap in track.gaps() {
                if issues.len() >= 510 {
                    break;
                }
                issues.push(DiagnosticIssue { code: "unresolved_gap".into(), message: "Unavailable evidence remains a gap; standard export requires explicit resolution.".into(), axis: Some(track.axis()), range: Some(*gap) });
            }
        }
        for region in &protected {
            if issues.len() >= 510 {
                break;
            }
            issues.push(DiagnosticIssue {
                code: "protected_region".into(),
                message: "Selective merge preserves this region and its interpolation anchors."
                    .into(),
                axis: region.axis,
                range: Some(region.range),
            });
        }
        let reviews = candidate
            .as_ref()
            .map(|candidate| candidate.review.as_slice())
            .or_else(|| review_state.as_ref().map(|state| state.flags.as_slice()))
            .unwrap_or(&[]);
        for flag in reviews {
            if issues.len() >= 510 {
                break;
            }
            let label = if review_state
                .as_ref()
                .is_some_and(|state| state.conservative)
            {
                "Conservative unresolved review envelope; no human resolution recorded"
            } else if candidate.is_some() {
                "Candidate review required"
            } else {
                "Unresolved revision review"
            };
            issues.push(DiagnosticIssue {
                code: flag.reason.as_str().into(),
                message: format!("{label}: {} ({:?}).", flag.reason.as_str(), flag.evidence),
                axis: flag.axis,
                range: Some(flag.range),
            });
        }
        if issues.len() >= 510 {
            issues.push(DiagnosticIssue {
                code: "diagnostics_truncated".into(),
                message: "Diagnostic output reached its bounded page limit.".into(),
                axis: None,
                range: None,
            });
        }
        Ok(ResponseBody::Diagnostics(DiagnosticsReport {
            project_id: snapshot.project_id,
            revision: snapshot.revision,
            candidate_id: candidate_id.cloned(),
            base_revision: candidate.as_ref().map(|candidate| candidate.base_revision),
            protected_regions: protected,
            issues,
        }))
    }
}

fn protected_regions(db: &Connection, project: &ProjectId) -> PResult<Vec<ProtectedRegion>> {
    let raw: String = db
        .query_row(
            "SELECT protected FROM projects WHERE id=?1",
            [project.as_str()],
            |row| row.get(0),
        )
        .map_err(internal)?;
    decode(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        engine: Arc<Engine>,
        session: SessionId,
        token: String,
        project: ProjectId,
        dir: tempfile::TempDir,
    }
    impl Fixture {
        fn request(&self, command: Command, revision: Option<u64>) -> Request {
            Request::new(id!(RequestId), command)
                .with_session(self.session.clone())
                .with_auth_token(self.token.clone())
                .in_project(self.project.clone(), revision.map(RevisionId::new))
        }
        fn call(&self, command: Command, revision: Option<u64>) -> PResult<ResponseBody> {
            self.engine.handle(self.request(command, revision)).result
        }
        fn import(&self, bytes: &[u8], revision: u64) -> CandidateSnapshot {
            let path = self
                .dir
                .path()
                .join(format!("{}.funscript", Uuid::new_v4()));
            fs::write(&path, bytes).unwrap();
            match self
                .call(Command::ImportFunscript { path }, Some(revision))
                .unwrap()
            {
                ResponseBody::Candidate(candidate) => candidate,
                _ => panic!("candidate expected"),
            }
        }
    }
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("worker");
        fs::write(&binary, b"nonexecuted editing test worker").unwrap();
        let engine = Engine::open(EngineConfig::new(dir.path().join("state"), binary)).unwrap();
        let (session, token) = match engine
            .handle(Request::new(
                id!(RequestId),
                Command::Pair {
                    client_name: "editing tests".into(),
                    pairing_token: engine.token.clone(),
                },
            ))
            .result
            .unwrap()
        {
            ResponseBody::Paired {
                session,
                auth_token,
            } => (session, auth_token),
            _ => panic!(),
        };
        let project = match engine
            .handle(
                Request::new(
                    id!(RequestId),
                    Command::CreateProject {
                        name: "editing".into(),
                    },
                )
                .with_session(session.clone())
                .with_auth_token(token.clone()),
            )
            .result
            .unwrap()
        {
            ResponseBody::Project(project) => project.project_id,
            _ => panic!(),
        };
        Fixture {
            engine,
            session,
            token,
            project,
            dir,
        }
    }
    // Stage only a proposal; the public command still checks all authority and revision rules.
    fn commit_program(f: &Fixture, revision: u64, program: MotionProgram) -> Command {
        let candidate_id = id!(CandidateId);
        let db = f.engine.db.lock().unwrap();
        let reference = crate::authority::motion_state::save(&db, &program).unwrap();
        db.execute(
            "INSERT INTO candidates(id,project,base_revision,program,job,lineage,review) VALUES(?1,?2,?3,?4,NULL,'[]','[]')",
            params![candidate_id.as_str(), f.project.as_str(), revision as i64, reference],
        ).unwrap();
        Command::CommitCandidate { candidate_id }
    }
    #[test]
    fn funscript_import_is_revision_bound_durable_candidate_and_diagnostics_are_read_only() {
        let f = fixture();
        let path = f.dir.path().join("input.funscript");
        fs::write(
            &path,
            br#"{"actions":[{"at":0,"pos":20},{"at":1000,"pos":80}]}"#,
        )
        .unwrap();
        let request = f.request(Command::ImportFunscript { path: path.clone() }, Some(0));
        let first = f.engine.handle(request.clone()).result.unwrap();
        fs::write(path, b"changed original").unwrap();
        assert_eq!(
            encode(&first).unwrap(),
            encode(&f.engine.handle(request).result.unwrap()).unwrap()
        );
        let ResponseBody::Candidate(candidate) = first else {
            panic!()
        };
        assert_eq!(candidate.review[0].reason.as_str(), "imported_motion");
        let diagnostic = f.request(
            Command::Diagnostics {
                candidate_id: Some(candidate.candidate_id.clone()),
            },
            None,
        );
        for _ in 0..3 {
            f.engine.handle(diagnostic.clone()).result.unwrap();
        }
        let db = f.engine.db.lock().unwrap();
        assert_eq!(
            project_snapshot(&db, &f.project).unwrap().revision.value(),
            0
        );
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM requests WHERE request=?1",
                [diagnostic.request_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        assert!(project_state(&db, &f.project)
            .unwrap()
            .program
            .tracks()
            .is_empty());
    }
    #[test]
    fn import_rejects_missing_scope_stale_revision_and_malformed_signal() {
        let f = fixture();
        for raw in [
            br#"{"actions":[{"at":0,"pos":101}]}"#.as_slice(),
            br#"{"actions":[{"at":1,"pos":20},{"at":1,"pos":80}]}"#.as_slice(),
            br#"{"actions":[{"at":18446744073709551615,"pos":20}]}"#.as_slice(),
        ] {
            let path = f.dir.path().join("invalid.funscript");
            fs::write(&path, raw).unwrap();
            assert!(f.call(Command::ImportFunscript { path }, Some(0)).is_err());
        }
        let path = f.dir.path().join("valid.funscript");
        fs::write(&path, br#"{"actions":[{"at":0,"pos":20}]}"#).unwrap();
        assert_eq!(
            f.call(Command::ImportFunscript { path: path.clone() }, Some(99))
                .unwrap_err()
                .code,
            ErrorCode::RevisionConflict
        );
        let db = f.engine.db.lock().unwrap();
        db.execute(
            "DELETE FROM grants WHERE session=?1 AND project=?2 AND scope=?3",
            params![
                f.session.as_str(),
                f.project.as_str(),
                scope_name(Scope::ImportSource)
            ],
        )
        .unwrap();
        drop(db);
        assert_eq!(
            f.call(Command::ImportFunscript { path }, Some(0))
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
    }
    #[test]
    fn selective_merge_preserves_protection_and_supports_undo_redo() {
        let f = fixture();
        let original = f.import(br#"{"actions":[{"at":0,"pos":0},{"at":100,"pos":100},{"at":200,"pos":20},{"at":300,"pos":80}]}"#, 0);
        f.call(
            Command::CommitCandidate {
                candidate_id: original.candidate_id,
            },
            Some(0),
        )
        .unwrap();
        let region = ProtectedRegion {
            axis: Some(Axis::Stroke),
            range: TimeRange::new(
                ProjectTime::from_nanos(20_000_000),
                ProjectTime::from_nanos(80_000_000),
            )
            .unwrap(),
        };
        f.call(
            Command::SetProtectedRegions {
                regions: vec![region],
            },
            Some(1),
        )
        .unwrap();
        let incoming = f.import(br#"{"actions":[{"at":0,"pos":100},{"at":50,"pos":50},{"at":100,"pos":0},{"at":200,"pos":90},{"at":300,"pos":10}]}"#, 2);
        let merge = f.request(
            Command::MergeCandidate {
                candidate_id: incoming.candidate_id.clone(),
                axes: vec![Axis::Stroke],
                range: None,
            },
            Some(2),
        );
        let ResponseBody::Project(merged) = f.engine.handle(merge.clone()).result.unwrap() else {
            panic!()
        };
        let merged_program = f.engine.motion_store.read(&merged.motion.program).unwrap();
        assert_eq!(
            merged_program.track(Axis::Stroke).unwrap().actions()[0]
                .position()
                .value(),
            0.0
        );
        assert_eq!(
            merged_program.track(Axis::Stroke).unwrap().actions()[1]
                .position()
                .value(),
            1.0
        );
        assert_eq!(
            merged_program.track(Axis::Stroke).unwrap().actions()[2]
                .position()
                .value(),
            0.9
        );
        assert_eq!(
            encode(&ResponseBody::Project(merged)).unwrap(),
            encode(&f.engine.handle(merge).result.unwrap()).unwrap()
        );
        f.call(Command::Undo, Some(3)).unwrap();
        f.call(Command::Redo, Some(4)).unwrap();
        assert_eq!(
            f.call(
                Command::MergeCandidate {
                    candidate_id: incoming.candidate_id,
                    axes: vec![Axis::Stroke],
                    range: None
                },
                Some(5)
            )
            .unwrap_err()
            .code,
            ErrorCode::RevisionConflict
        );
    }
    #[test]
    fn candidate_review_survives_rebase_and_revocation_denies_replay() {
        let f = fixture();
        let candidate = f.import(
            br#"{"actions":[{"at":0,"pos":20},{"at":1000,"pos":80}]}"#,
            0,
        );
        let request = f.request(
            Command::RebaseCandidate {
                candidate_id: candidate.candidate_id,
            },
            Some(0),
        );
        let ResponseBody::Candidate(rebased) = f.engine.handle(request.clone()).result.unwrap()
        else {
            panic!()
        };
        assert_eq!(
            encode(&rebased.review).unwrap(),
            encode(&candidate.review).unwrap()
        );
        f.engine
            .db
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM grants WHERE session=?1 AND project=?2",
                params![f.session.as_str(), f.project.as_str()],
            )
            .unwrap();
        assert_eq!(
            f.engine.handle(request).result.unwrap_err().code,
            ErrorCode::Forbidden
        );
    }
    #[test]
    fn explicit_axis_export_is_correct_revision_bound_and_has_axis_receipt() {
        let f = fixture();
        let make_track = |axis, position| {
            MotionTrack::new(
                axis,
                vec![
                    MotionAction::new(
                        ProjectTime::ZERO,
                        NormalizedPosition::new(position).unwrap(),
                        EvidenceKind::Synthesized,
                    )
                    .unwrap(),
                    MotionAction::new(
                        ProjectTime::from_nanos(1_000_000_000),
                        NormalizedPosition::new(position).unwrap(),
                        EvidenceKind::Synthesized,
                    )
                    .unwrap(),
                ],
            )
            .unwrap()
        };
        let program = MotionProgram::new(vec![
            make_track(Axis::Stroke, 0.2),
            make_track(Axis::Sway, 0.8),
        ])
        .unwrap();
        f.call(commit_program(&f, 0, program), Some(0)).unwrap();
        let ambiguous = f.dir.path().join("ambiguous.funscript");
        assert_eq!(
            f.call(
                Command::Export {
                    path: ambiguous.clone()
                },
                None
            )
            .unwrap_err()
            .code,
            ErrorCode::Unsupported
        );
        assert!(!ambiguous.exists());
        let path = f.dir.path().join("selected.sway.funscript");
        let command = Command::ExportAxis {
            path: path.clone(),
            axis: Axis::Sway,
        };
        assert_eq!(
            f.call(command.clone(), None).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            f.call(command.clone(), Some(0)).unwrap_err().code,
            ErrorCode::RevisionConflict
        );
        assert!(!path.exists());
        let request = f.request(command, Some(1));
        let response = f.engine.handle(request.clone()).result.unwrap();
        let ResponseBody::AxisExported { axis, revision, .. } = &response else {
            panic!()
        };
        assert_eq!(*axis, Axis::Sway);
        assert_eq!(revision.value(), 1);
        let output: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(output["actions"][0]["pos"], 80);
        assert!(output.get("axis").is_none());
        assert!(output.get("lineage").is_none());
        assert_eq!(
            encode(&response).unwrap(),
            encode(&f.engine.handle(request.clone()).result.unwrap()).unwrap()
        );
        let db = f.engine.db.lock().unwrap();
        let axis: String = db
            .query_row(
                "SELECT axis FROM exports WHERE session=?1 AND request=?2",
                params![f.session.as_str(), request.request_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(axis, "sway");
        db.execute(
            "DELETE FROM grants WHERE session=?1 AND project=?2 AND scope=?3",
            params![
                f.session.as_str(),
                f.project.as_str(),
                scope_name(Scope::Export)
            ],
        )
        .unwrap();
        drop(db);
        assert_eq!(
            f.engine.handle(request).result.unwrap_err().code,
            ErrorCode::Forbidden
        );
    }

    #[test]
    fn axis_export_rejects_absent_empty_gapped_and_quantization_collisions() {
        assert!(neutral_export_axis(&MotionProgram::default(), Axis::Stroke).is_err());
        let empty = MotionProgram::new(vec![MotionTrack::new(Axis::Yaw, vec![]).unwrap()]).unwrap();
        assert!(neutral_export_axis(&empty, Axis::Yaw).is_err());
        let action = |ns| {
            MotionAction::new(
                ProjectTime::from_nanos(ns),
                NormalizedPosition::new(0.5).unwrap(),
                EvidenceKind::Synthesized,
            )
            .unwrap()
        };
        let collision = MotionProgram::new(vec![MotionTrack::new(
            Axis::Pitch,
            vec![action(0), action(1)],
        )
        .unwrap()])
        .unwrap();
        assert!(neutral_export_axis(&collision, Axis::Pitch).is_err());
        let valid = MotionTrack::new(Axis::Stroke, vec![action(0), action(1_000_000)]).unwrap();
        let gapped = MotionTrack::with_gaps(
            Axis::Surge,
            vec![action(0), action(10_000_000)],
            vec![TimeRange::new(
                ProjectTime::from_nanos(1_000_000),
                ProjectTime::from_nanos(2_000_000),
            )
            .unwrap()],
        )
        .unwrap();
        let program = MotionProgram::new(vec![valid, gapped]).unwrap();
        assert!(neutral_export_axis(&program, Axis::Surge).is_err());
        assert!(neutral_export_axis(&program, Axis::Stroke).is_ok());
        assert!(neutral_export_axis(&program, Axis::Roll).is_err());
    }

    fn project_review_issues(f: &Fixture) -> Vec<DiagnosticIssue> {
        match f
            .call(Command::Diagnostics { candidate_id: None }, None)
            .unwrap()
        {
            ResponseBody::Diagnostics(report) => report.issues,
            _ => panic!("diagnostics expected"),
        }
    }
    fn reopen_fixture(f: Fixture) -> Fixture {
        let Fixture {
            engine,
            session,
            token,
            project,
            dir,
        } = f;
        let config = engine.config.clone();
        drop(engine);
        let engine = Engine::open(config).unwrap();
        Fixture {
            engine,
            session,
            token,
            project,
            dir,
        }
    }
    #[test]
    fn committed_reviews_and_explicit_history_edges_survive_restart_and_replay() {
        let f = fixture();
        let candidate = f.import(
            br#"{"actions":[{"at":0,"pos":20},{"at":1000,"pos":80}]}"#,
            0,
        );
        let commit = f.request(
            Command::CommitCandidate {
                candidate_id: candidate.candidate_id.clone(),
            },
            Some(0),
        );
        let committed = f.engine.handle(commit.clone()).result.unwrap();
        assert!(project_review_issues(&f)
            .iter()
            .any(|issue| issue.code == "imported_motion"));
        f.call(Command::SetProtectedRegions { regions: vec![] }, Some(1))
            .unwrap();
        f.call(Command::Undo, Some(2)).unwrap();
        f.call(Command::Redo, Some(3)).unwrap();
        let f = reopen_fixture(f);
        assert!(project_review_issues(&f)
            .iter()
            .any(|issue| issue.code == "imported_motion"));
        assert_eq!(
            encode(&committed).unwrap(),
            encode(&f.engine.handle(commit).result.unwrap()).unwrap()
        );
        let db = f.engine.db.lock().unwrap();
        let restored: (i64,i64,i64) = db.query_row(
            "SELECT parent_revision,restored_from_revision,restored_history_position FROM revision_lineage WHERE project=?1 AND revision=4",
            [f.project.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).unwrap();
        assert_eq!(restored, (3, 2, 2));
        let origin: String = db
            .query_row(
                "SELECT candidate FROM revision_lineage WHERE project=?1 AND revision=1",
                [f.project.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(origin, candidate.candidate_id.as_str());
        assert_eq!(
            project_snapshot(&db, &f.project).unwrap().revision.value(),
            4
        );
    }
    #[test]
    fn partial_merge_review_excludes_protected_and_omitted_axes() {
        let f = fixture();
        let action = |at, pos| {
            MotionAction::new(
                ProjectTime::from_nanos(at),
                NormalizedPosition::new(pos).unwrap(),
                EvidenceKind::Synthesized,
            )
            .unwrap()
        };
        let original = MotionProgram::new(vec![
            MotionTrack::new(
                Axis::Stroke,
                vec![
                    action(0, 0.1),
                    action(100, 0.2),
                    action(200, 0.3),
                    action(300, 0.4),
                ],
            )
            .unwrap(),
            MotionTrack::new(Axis::Sway, vec![action(0, 0.2), action(300, 0.2)]).unwrap(),
        ])
        .unwrap();
        f.call(commit_program(&f, 0, original), Some(0)).unwrap();
        f.call(
            Command::SetProtectedRegions {
                regions: vec![ProtectedRegion {
                    axis: Some(Axis::Stroke),
                    range: TimeRange::new(ProjectTime::ZERO, ProjectTime::from_nanos(100)).unwrap(),
                }],
            },
            Some(1),
        )
        .unwrap();
        let incoming = MotionProgram::new(vec![
            MotionTrack::new(
                Axis::Stroke,
                vec![
                    action(0, 0.9),
                    action(100, 0.9),
                    action(150, 0.8),
                    action(200, 0.9),
                    action(300, 0.9),
                ],
            )
            .unwrap(),
            MotionTrack::new(Axis::Sway, vec![action(0, 0.9), action(300, 0.9)]).unwrap(),
        ])
        .unwrap();
        let flag = |axis, end, code| ReviewFlag {
            axis,
            range: TimeRange::new(ProjectTime::ZERO, ProjectTime::from_nanos(end)).unwrap(),
            reason: ReviewReason::new(code).unwrap(),
            evidence: EvidenceKind::Synthesized,
            confidence: None,
        };
        let reviews = vec![
            flag(None, 301, "selected_motion"),
            flag(Some(Axis::Stroke), 100, "protected_unused"),
            flag(Some(Axis::Sway), 301, "omitted_axis"),
        ];
        let candidate = id!(CandidateId);
        {
            let db = f.engine.db.lock().unwrap();
            let reference = crate::authority::motion_state::save(&db, &incoming).unwrap();
            db.execute("INSERT INTO candidates(id,project,base_revision,program,job,lineage,review) VALUES(?1,?2,2,?3,NULL,'[]',?4)",
                params![candidate.as_str(),f.project.as_str(),reference,encode(&reviews).unwrap()]).unwrap();
        }
        f.call(
            Command::MergeCandidate {
                candidate_id: candidate,
                axes: vec![Axis::Stroke],
                range: None,
            },
            Some(2),
        )
        .unwrap();
        let issues = project_review_issues(&f);
        let contributed = issues
            .iter()
            .find(|issue| issue.code == "selected_motion")
            .unwrap();
        assert_eq!(contributed.axis, Some(Axis::Stroke));
        assert!(contributed.range.unwrap().start().as_nanos() > 100);
        assert!(contributed.message.contains("Conservative"));
        assert!(!issues
            .iter()
            .any(|issue| issue.code == "protected_unused" || issue.code == "omitted_axis"));
    }
    #[test]
    fn review_snapshot_overflow_rolls_back_revision_history_and_candidate_commit() {
        let f = fixture();
        let first = f.import(
            br#"{"actions":[{"at":0,"pos":20},{"at":1000,"pos":80}]}"#,
            0,
        );
        f.call(
            Command::CommitCandidate {
                candidate_id: first.candidate_id,
            },
            Some(0),
        )
        .unwrap();
        let incoming = f.import(
            br#"{"actions":[{"at":0,"pos":80},{"at":1000,"pos":20}]}"#,
            1,
        );
        let reviews = vec![incoming.review[0].clone(); 1024];
        f.engine
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE candidates SET review=?2 WHERE id=?1",
                params![incoming.candidate_id.as_str(), encode(&reviews).unwrap()],
            )
            .unwrap();
        let request = f.request(
            Command::MergeCandidate {
                candidate_id: incoming.candidate_id.clone(),
                axes: vec![Axis::Stroke],
                range: None,
            },
            Some(1),
        );
        assert_eq!(
            f.engine.handle(request.clone()).result.unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
        let db = f.engine.db.lock().unwrap();
        assert_eq!(
            project_snapshot(&db, &f.project).unwrap().revision.value(),
            1
        );
        let counts: (i64,i64,i64) = db.query_row(
            "SELECT (SELECT COUNT(*) FROM revisions WHERE project=?1),(SELECT COUNT(*) FROM edit_states WHERE project=?1),(SELECT COUNT(*) FROM revision_lineage WHERE project=?1)",
            [f.project.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).unwrap();
        assert_eq!(counts, (2, 2, 2));
        let committed: Option<i64> = db
            .query_row(
                "SELECT committed_revision FROM candidates WHERE id=?1",
                [incoming.candidate_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(committed, None);
        let remembered: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM requests WHERE request=?1",
                [request.request_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remembered, 0);
    }
    #[test]
    fn legacy_history_is_explicitly_unknown_instead_of_inferred() {
        let f = fixture();
        let candidate = f.import(
            br#"{"actions":[{"at":0,"pos":20},{"at":1000,"pos":80}]}"#,
            0,
        );
        f.call(
            Command::CommitCandidate {
                candidate_id: candidate.candidate_id,
            },
            Some(0),
        )
        .unwrap();
        {
            let db = f.engine.db.lock().unwrap();
            db.execute("DELETE FROM history_lineage", []).unwrap();
            db.execute("DELETE FROM revision_lineage", []).unwrap();
        }
        let f = reopen_fixture(f);
        assert!(project_review_issues(&f)
            .iter()
            .any(|issue| issue.code == "review_history_unknown"));
        assert!(project_review_issues(&f)
            .iter()
            .any(|issue| issue.code == "imported_motion"));
        f.call(Command::Undo, Some(1)).unwrap();
        assert!(project_review_issues(&f)
            .iter()
            .any(|issue| issue.code == "review_history_unknown"));
        let db = f.engine.db.lock().unwrap();
        let edge: (Option<i64>,i64) = db.query_row(
            "SELECT restored_from_revision,restored_history_position FROM revision_lineage WHERE project=?1 AND revision=2",
            [f.project.as_str()], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(edge, (None, 0));
    }
    #[test]
    fn source_kind_prevents_funscript_preview_before_worker_admission() {
        let f = fixture();
        f.import(
            br#"{"actions":[{"at":0,"pos":20},{"at":1000,"pos":80}]}"#,
            0,
        );
        let ResponseBody::Project(project) = f.call(Command::GetSnapshot, None).unwrap() else {
            panic!()
        };
        assert_eq!(project.sources.len(), 1);
        assert_eq!(project.sources[0].kind, SourceKind::Funscript);
        let source = project.sources[0].source_version.clone();
        let result = f.call(
            Command::PreviewFrame {
                context: FrameContext {
                    source_version: source,
                    source_placement: SourcePlacementId::new("source").unwrap(),
                    frame: FrameId::new(0),
                    transform: TransformId::new("source-identity").unwrap(),
                    seek_generation: 0,
                    request_generation: 0,
                },
                source_time: SourceTimestamp::new(0, 1).unwrap(),
                model_path: None,
                model_input: None,
            },
            None,
        );
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidRequest);
        assert!(f.engine.workers.lock().unwrap().is_empty());
    }

    #[test]
    fn branching_history_retains_immutable_restoration_edges() {
        let f = fixture();
        let candidate = f.import(
            br#"{"actions":[{"at":0,"pos":20},{"at":1000,"pos":80}]}"#,
            0,
        );
        f.call(
            Command::CommitCandidate {
                candidate_id: candidate.candidate_id,
            },
            Some(0),
        )
        .unwrap();
        f.call(Command::SetProtectedRegions { regions: vec![] }, Some(1))
            .unwrap();
        f.call(Command::Undo, Some(2)).unwrap();
        let ResponseBody::Project(snapshot) = f.call(Command::GetSnapshot, None).unwrap() else {
            panic!()
        };
        assert_eq!(snapshot.revision.value(), 3);
        // Branch through the authored-edit API so existing review lineage is retained,
        // rather than inventing a new import with unrelated provenance.
        let program = f
            .engine
            .motion_store
            .read(&snapshot.motion.program)
            .unwrap();
        let tracks = program
            .tracks()
            .iter()
            .map(|track| {
                serde_json::json!({
                    "axis": track.axis(),
                    "actions": track.actions().iter().map(|action| serde_json::json!({
                        "time": action.time().as_nanos(),
                        "position": action.position().value(),
                    })).collect::<Vec<_>>(),
                    "gaps": track.gaps(),
                })
            })
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&serde_json::json!({"tracks": tracks})).unwrap();
        let sha256 = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&bytes));
        let ResponseBody::Transfer(lease) = f
            .call(
                Command::BeginEditUpload {
                    byte_len: bytes.len() as u64,
                    sha256,
                    label: "branch".into(),
                },
                Some(3),
            )
            .unwrap()
        else {
            panic!()
        };
        let handshake = BulkHandshake {
            version: PROTOCOL_VERSION,
            session: f.session.clone(),
            auth_token: f.token.clone(),
            lease_id: lease.lease_id.clone(),
            engine_epoch: lease.engine_epoch.clone(),
        };
        assert_eq!(
            f.engine.bulk_upload(&handshake, 0, &bytes).unwrap(),
            bytes.len() as u64
        );
        let ResponseBody::Candidate(branch) = f
            .call(
                Command::FinishEditUpload {
                    lease_id: lease.lease_id,
                },
                None,
            )
            .unwrap()
        else {
            panic!()
        };
        f.call(
            Command::CommitCandidate {
                candidate_id: branch.candidate_id,
            },
            Some(3),
        )
        .unwrap();
        let f = reopen_fixture(f);
        assert!(
            project_review_issues(&f)
                .iter()
                .any(|issue| issue.code == "imported_motion"
                    && issue.message.contains("Conservative"))
        );
        let db = f.engine.db.lock().unwrap();
        let revisions: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM revision_lineage WHERE project=?1",
                [f.project.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revisions, 5);
        let restored: i64 = db.query_row("SELECT restored_from_revision FROM revision_lineage WHERE project=?1 AND revision=3",
            [f.project.as_str()],|row|row.get(0)).unwrap();
        assert_eq!(restored, 1);
        let branch: i64 = db
            .query_row(
                "SELECT revision FROM history_lineage WHERE project=?1 AND position=2",
                [f.project.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(branch, 4);
        let old: String = db
            .query_row(
                "SELECT operation FROM revision_lineage WHERE project=?1 AND revision=2",
                [f.project.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old, "protection");
    }
    #[test]
    fn synthetic_source_kind_backfills_from_trusted_job_and_media_stays_media() {
        let f = fixture();
        let input = GenerationInput::Text {
            prompt: "stroke sine for one second".into(),
        };
        let source = f.engine.materialize_input(&input).unwrap();
        let path = f.dir.path().join("ordinary.fixture");
        fs::write(&path, b"ordinary imported media").unwrap();
        let ResponseBody::Source {
            source_version: media,
        } = f.call(Command::ImportSource { path }, None).unwrap()
        else {
            panic!()
        };
        let job = id!(JobId);
        let attempt = id!(AttemptId);
        let reference = ArtifactReference {
            identity: source.identity.clone(),
            path: source.path.clone(),
        };
        let manifest = WorkerRequest {
            version: PROTOCOL_VERSION,
            job_id: job.clone(),
            attempt_id: attempt.clone(),
            project_id: f.project.clone(),
            base_revision: RevisionId::new(0),
            source: source.clone(),
            output_dir: f
                .engine
                .config
                .state_dir
                .join("attempts")
                .join(attempt.as_str()),
            operation: WorkerOperation::GenerateInput { input },
            budget: ResourceBudget {
                memory_bytes: 1024,
                output_bytes: 1024,
                wall_time_ms: 1000,
                cpu_threads: 1,
            },
            dependencies: vec![source.identity.clone()],
            tools: WorkerTools {
                ffmpeg: reference.clone(),
                ffprobe: reference,
                onnx_runtime: None,
            },
        };
        {
            let db = f.engine.db.lock().unwrap();
            db.execute("INSERT INTO sources(project,version,path,original,label,sha256,bytes,kind) VALUES(?1,?2,?3,'','misleading.mp4',?4,?5,'media')",
                params![f.project.as_str(),source.source_version.as_str(),source.path.to_string_lossy(),source.identity.sha256,source.identity.byte_len as i64]).unwrap();
            db.execute("INSERT INTO jobs(id,attempt,project,base_revision,session,source,state,manifest) VALUES(?1,?2,?3,0,?4,?5,'completed',?6)",
                params![job.as_str(),attempt.as_str(),f.project.as_str(),f.session.as_str(),source.source_version.as_str(),encode(&manifest).unwrap()]).unwrap();
        }
        let f = reopen_fixture(f);
        let db = f.engine.db.lock().unwrap();
        let project = project_snapshot(&db, &f.project).unwrap();
        assert_eq!(
            project
                .sources
                .iter()
                .find(|item| item.source_version == source.source_version)
                .unwrap()
                .kind,
            SourceKind::GenerationInput
        );
        assert_eq!(
            project
                .sources
                .iter()
                .find(|item| item.source_version == media)
                .unwrap()
                .kind,
            SourceKind::Media
        );
        assert_eq!(
            source_kinds::require_media(&db, &f.project, &source.source_version)
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn singleton_merge_review_covers_retained_anchor_interpolation() {
        let f = fixture();
        let original = f.import(br#"{"actions":[{"at":0,"pos":10},{"at":100,"pos":20},{"at":200,"pos":30},{"at":300,"pos":40}]}"#,0);
        f.call(
            Command::CommitCandidate {
                candidate_id: original.candidate_id,
            },
            Some(0),
        )
        .unwrap();
        let incoming = MotionProgram::new(vec![MotionTrack::new(
            Axis::Stroke,
            vec![MotionAction::new(
                ProjectTime::from_nanos(150_000_000),
                NormalizedPosition::new(0.8).unwrap(),
                EvidenceKind::Synthesized,
            )
            .unwrap()],
        )
        .unwrap()])
        .unwrap();
        let range = TimeRange::new(
            ProjectTime::from_nanos(100_000_000),
            ProjectTime::from_nanos(200_000_000),
        )
        .unwrap();
        let review = vec![ReviewFlag {
            axis: None,
            range,
            reason: ReviewReason::new("singleton_motion").unwrap(),
            evidence: EvidenceKind::Synthesized,
            confidence: None,
        }];
        let candidate = id!(CandidateId);
        {
            let db = f.engine.db.lock().unwrap();
            let reference = crate::authority::motion_state::save(&db, &incoming).unwrap();
            db.execute("INSERT INTO candidates(id,project,base_revision,program,job,lineage,review) VALUES(?1,?2,1,?3,NULL,'[]',?4)",
                params![candidate.as_str(),f.project.as_str(),reference,encode(&review).unwrap()]).unwrap();
        }
        f.call(
            Command::MergeCandidate {
                candidate_id: candidate,
                axes: vec![Axis::Stroke],
                range: Some(range),
            },
            Some(1),
        )
        .unwrap();
        let f = reopen_fixture(f);
        let issues = project_review_issues(&f);
        let issue = issues
            .iter()
            .find(|issue| issue.code == "singleton_motion")
            .unwrap();
        assert_eq!(issue.axis, Some(Axis::Stroke));
        assert!(issue.message.contains("Conservative"));
        assert_eq!(
            issue.range.unwrap(),
            TimeRange::new(
                ProjectTime::from_nanos(100_000_001),
                ProjectTime::from_nanos(200_000_000)
            )
            .unwrap()
        );
        assert!(issue
            .range
            .unwrap()
            .contains(ProjectTime::from_nanos(125_000_000)));
        let prior = issues
            .iter()
            .find(|issue| issue.code == "imported_motion")
            .unwrap();
        assert_eq!(prior.range.unwrap().start(), ProjectTime::ZERO);
        assert_eq!(
            prior.range.unwrap().end(),
            ProjectTime::from_nanos(300_000_001)
        );
    }
}
