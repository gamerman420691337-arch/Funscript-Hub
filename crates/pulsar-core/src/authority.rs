use crate::{ClientId, CoreError, GrantId, ProjectId, RevisionId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Query,
    Edit,
    ManageJobs,
    Export,
    Playback,
    ManageGrants,
    OverrideProtection,
    NativeExtensions,
}

/// This value expresses a grant already authenticated by the engine. Receiving
/// a serialized grant from an untrusted client must never grant authority.
/// Absence of a project is an explicit user-wide grant, not a default scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CapabilityGrant {
    id: GrantId,
    client: ClientId,
    project: Option<ProjectId>,
    capabilities: BTreeSet<Capability>,
    revoked: bool,
}

impl CapabilityGrant {
    pub fn new(
        id: GrantId,
        client: ClientId,
        project: Option<ProjectId>,
        capabilities: impl IntoIterator<Item = Capability>,
    ) -> Self {
        Self {
            id,
            client,
            project,
            capabilities: capabilities.into_iter().collect(),
            revoked: false,
        }
    }
    pub fn id(&self) -> &GrantId {
        &self.id
    }
    pub fn client(&self) -> &ClientId {
        &self.client
    }
    pub fn project(&self) -> Option<&ProjectId> {
        self.project.as_ref()
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked
    }
    pub fn revoke(&mut self) {
        self.revoked = true;
    }
    pub fn allows(
        &self,
        client: &ClientId,
        project: Option<&ProjectId>,
        capability: Capability,
    ) -> bool {
        !self.revoked
            && self.client == *client
            && (self.project.is_none() || self.project.as_ref() == project)
            && self.capabilities.contains(&capability)
    }
    pub fn require(
        &self,
        client: &ClientId,
        project: Option<&ProjectId>,
        capability: Capability,
    ) -> Result<(), CoreError> {
        if self.allows(client, project, capability) {
            Ok(())
        } else {
            Err(CoreError::PermissionDenied)
        }
    }
}

pub fn ensure_revision(expected: RevisionId, actual: RevisionId) -> Result<(), CoreError> {
    if expected == actual {
        Ok(())
    } else {
        Err(CoreError::RevisionConflict {
            expected: expected.value(),
            actual: actual.value(),
        })
    }
}
