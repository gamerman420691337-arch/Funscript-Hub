use crate::{
    ensure_revision, AttemptId, Axis, CandidateId, Capability, CapabilityGrant, ClientId,
    CoreError, JobId, MotionProgram, MotionTrack, ProjectId, RevisionId, TimeRange,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectedRegion {
    pub axis: Option<Axis>,
    pub range: TimeRange,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSnapshot {
    pub project: ProjectId,
    pub revision: RevisionId,
    pub program: MotionProgram,
    pub protected_regions: Vec<ProtectedRegion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ProjectEdit {
    ReplaceProgram(MotionProgram),
    ReplaceTrack(MotionTrack),
    SetProtection(Vec<ProtectedRegion>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditKind {
    Edit,
    Undo,
    Redo,
    CandidateCommit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditReceipt {
    pub revision: RevisionId,
    pub actor: ClientId,
    pub kind: EditKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionCandidate {
    pub id: CandidateId,
    pub project: ProjectId,
    pub base_revision: RevisionId,
    pub program: MotionProgram,
    pub job: JobId,
    pub attempt: AttemptId,
}

#[derive(Clone, Debug)]
struct HistoryEntry {
    program: MotionProgram,
    protection: Vec<ProtectedRegion>,
}

/// Pure project state. The engine wraps a successful transition, actor receipt,
/// request identity, artifacts and history in one durable database transaction.
/// It must not acknowledge a successful in-memory transition as persistence.
#[derive(Clone, Debug)]
pub struct ProjectState {
    snapshot: ProjectSnapshot,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
}

impl ProjectState {
    pub fn new(project: ProjectId, program: MotionProgram) -> Self {
        Self::from_snapshot(ProjectSnapshot {
            project,
            revision: RevisionId::new(0),
            program,
            protected_regions: vec![],
        })
    }
    /// Restores a validated snapshot only. Durable undo history is restored by
    /// the engine from its transactional history, not invented by this method.
    pub fn from_snapshot(snapshot: ProjectSnapshot) -> Self {
        Self {
            snapshot,
            undo: vec![],
            redo: vec![],
        }
    }
    pub fn snapshot(&self) -> &ProjectSnapshot {
        &self.snapshot
    }

    pub fn apply_edit(
        &mut self,
        expected: RevisionId,
        grant: &CapabilityGrant,
        edit: ProjectEdit,
    ) -> Result<EditReceipt, CoreError> {
        let (program, protection) = match edit {
            ProjectEdit::ReplaceProgram(program) => {
                (program, self.snapshot.protected_regions.clone())
            }
            ProjectEdit::ReplaceTrack(track) => (
                self.snapshot.program.replaced_track(track)?,
                self.snapshot.protected_regions.clone(),
            ),
            ProjectEdit::SetProtection(protection) => (self.snapshot.program.clone(), protection),
        };
        self.commit_change(expected, grant, program, protection, EditKind::Edit)
    }

    pub fn commit_candidate(
        &mut self,
        expected: RevisionId,
        grant: &CapabilityGrant,
        candidate: &MotionCandidate,
    ) -> Result<EditReceipt, CoreError> {
        if candidate.project != self.snapshot.project {
            return Err(CoreError::CandidateMismatch);
        }
        ensure_revision(candidate.base_revision, self.snapshot.revision)?;
        self.commit_change(
            expected,
            grant,
            candidate.program.clone(),
            self.snapshot.protected_regions.clone(),
            EditKind::CandidateCommit,
        )
    }

    pub fn undo(
        &mut self,
        expected: RevisionId,
        grant: &CapabilityGrant,
    ) -> Result<EditReceipt, CoreError> {
        let prior = self.undo.last().ok_or(CoreError::EmptyHistory)?.clone();
        let revision = self.validate_change(expected, grant, &prior.program, &prior.protection)?;
        let previous = self.current_entry();
        self.snapshot.program = prior.program;
        self.snapshot.protected_regions = prior.protection;
        self.snapshot.revision = revision;
        self.undo.pop();
        self.redo.push(previous);
        Ok(EditReceipt {
            revision,
            actor: grant.client().clone(),
            kind: EditKind::Undo,
        })
    }

    pub fn redo(
        &mut self,
        expected: RevisionId,
        grant: &CapabilityGrant,
    ) -> Result<EditReceipt, CoreError> {
        let next = self.redo.last().ok_or(CoreError::EmptyHistory)?.clone();
        let revision = self.validate_change(expected, grant, &next.program, &next.protection)?;
        let previous = self.current_entry();
        self.snapshot.program = next.program;
        self.snapshot.protected_regions = next.protection;
        self.snapshot.revision = revision;
        self.redo.pop();
        self.undo.push(previous);
        Ok(EditReceipt {
            revision,
            actor: grant.client().clone(),
            kind: EditKind::Redo,
        })
    }

    fn current_entry(&self) -> HistoryEntry {
        HistoryEntry {
            program: self.snapshot.program.clone(),
            protection: self.snapshot.protected_regions.clone(),
        }
    }
    fn commit_change(
        &mut self,
        expected: RevisionId,
        grant: &CapabilityGrant,
        program: MotionProgram,
        protection: Vec<ProtectedRegion>,
        kind: EditKind,
    ) -> Result<EditReceipt, CoreError> {
        let revision = self.validate_change(expected, grant, &program, &protection)?;
        self.undo.push(self.current_entry());
        self.redo.clear();
        self.snapshot.program = program;
        self.snapshot.protected_regions = protection;
        self.snapshot.revision = revision;
        Ok(EditReceipt {
            revision,
            actor: grant.client().clone(),
            kind,
        })
    }
    fn validate_change(
        &self,
        expected: RevisionId,
        grant: &CapabilityGrant,
        program: &MotionProgram,
        protection: &[ProtectedRegion],
    ) -> Result<RevisionId, CoreError> {
        grant.require(
            grant.client(),
            Some(&self.snapshot.project),
            Capability::Edit,
        )?;
        ensure_revision(expected, self.snapshot.revision)?;
        let can_override = grant.allows(
            grant.client(),
            Some(&self.snapshot.project),
            Capability::OverrideProtection,
        );
        if !can_override {
            if protection != self.snapshot.protected_regions {
                return Err(CoreError::PermissionDenied);
            }
            ensure_protected_regions_unchanged(
                &self.snapshot.program,
                program,
                &self.snapshot.protected_regions,
            )?;
        }
        self.snapshot.revision.checked_next()
    }
}

/// Conservative replacement protection: any changed track protects the union
/// of its old/new time spans. This never trusts a caller-supplied edited range.
/// Finer local edit operations can narrow the footprint with dedicated proofs.
pub fn ensure_protected_regions_unchanged(
    before: &MotionProgram,
    after: &MotionProgram,
    protected: &[ProtectedRegion],
) -> Result<(), CoreError> {
    if protected.is_empty() {
        return Ok(());
    }
    for axis in Axis::ALL {
        let old = before.track(axis);
        let new = after.track(axis);
        if old == new {
            continue;
        }
        for track in old.into_iter().chain(new) {
            for span in track
                .span()?
                .into_iter()
                .chain(track.gaps().iter().copied())
            {
                if protected.iter().any(|region| {
                    (region.axis.is_none() || region.axis == Some(axis))
                        && region.range.overlaps(span)
                }) {
                    return Err(CoreError::ProtectedRegion);
                }
            }
        }
    }
    Ok(())
}
