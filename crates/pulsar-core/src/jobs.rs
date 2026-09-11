use crate::{ArtifactId, AttemptId, CoreError, GrantId, JobId, RevisionId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Paused,
    CancelRequested,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobEvent {
    Start,
    Pause,
    Resume,
    RequestCancel,
    RevokeAuthority,
    WorkerCompleted,
    WorkerFailed,
    AcknowledgeCancel,
}

impl JobState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Completed | Self::Failed)
    }
    pub fn transition(self, event: JobEvent) -> Result<Self, CoreError> {
        use JobEvent::*;
        use JobState::*;
        match (self, event) {
            (Queued, Start) => Ok(Running),
            (Running, Pause) => Ok(Paused),
            (Paused, Resume) => Ok(Running),
            (Queued | Paused, RequestCancel | RevokeAuthority) => Ok(Cancelled),
            (Running, RequestCancel | RevokeAuthority) => Ok(CancelRequested),
            (CancelRequested, RequestCancel | RevokeAuthority) => Ok(CancelRequested),
            (Cancelled, RequestCancel | RevokeAuthority | AcknowledgeCancel) => Ok(Cancelled),
            (CancelRequested, AcknowledgeCancel | WorkerFailed) => Ok(Cancelled),
            (Running, WorkerCompleted) => Ok(Completed),
            (Queued | Running | Paused, WorkerFailed) => Ok(Failed),
            _ => Err(CoreError::InvalidTransition(format!(
                "job {self:?} cannot accept {event:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptDependencies {
    pub source_snapshots: Vec<ArtifactId>,
    pub model: ArtifactId,
    pub runtime: ArtifactId,
    pub configuration: ArtifactId,
    pub transforms: Vec<ArtifactId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointIdentity {
    pub job: JobId,
    pub attempt: AttemptId,
    pub dependencies: AttemptDependencies,
    pub checkpoint: ArtifactId,
}

impl CheckpointIdentity {
    /// Identity agreement is necessary, not sufficient: the engine must also
    /// validate checkpoint bytes, schema and the worker's resume capability.
    pub fn matches(
        &self,
        job: &JobId,
        attempt: &AttemptId,
        dependencies: &AttemptDependencies,
    ) -> bool {
        self.job == *job && self.attempt == *attempt && self.dependencies == *dependencies
    }
}

#[derive(Clone, Debug)]
pub struct JobLifecycle {
    job: JobId,
    attempt: AttemptId,
    authority: GrantId,
    base_revision: RevisionId,
    state: JobState,
}

impl JobLifecycle {
    pub fn new(
        job: JobId,
        attempt: AttemptId,
        authority: GrantId,
        base_revision: RevisionId,
    ) -> Self {
        Self {
            job,
            attempt,
            authority,
            base_revision,
            state: JobState::Queued,
        }
    }
    pub fn job(&self) -> &JobId {
        &self.job
    }
    pub fn attempt(&self) -> &AttemptId {
        &self.attempt
    }
    pub fn authority(&self) -> &GrantId {
        &self.authority
    }
    pub const fn base_revision(&self) -> RevisionId {
        self.base_revision
    }
    pub const fn state(&self) -> JobState {
        self.state
    }
    pub fn apply(&mut self, attempt: &AttemptId, event: JobEvent) -> Result<JobState, CoreError> {
        if attempt != &self.attempt {
            return Err(CoreError::InvalidTransition(
                "late event from another attempt".into(),
            ));
        }
        let next = self.state.transition(event)?;
        self.state = next;
        Ok(next)
    }
}
