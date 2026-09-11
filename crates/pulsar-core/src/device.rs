use crate::{ArtifactId, ClientId, CoreError, DeviceId, DeviceSessionId, ProjectId, RevisionId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfiguration {
    pub device: DeviceId,
    pub firmware: ArtifactId,
    pub driver: ArtifactId,
    pub transport: ArtifactId,
    pub profile: ArtifactId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualificationAuthority {
    OfficialRecord,
    LocalHumanAcceptance,
}

/// Construct only from an engine-verified official record or the human's
/// acceptance of mandatory local evidence. Clients cannot submit this value
/// as an authorization claim. No Deserialize implementation is provided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceQualification {
    configuration: DeviceConfiguration,
    record: ArtifactId,
    authority: QualificationAuthority,
    stopping_bound_ns: Option<u64>,
}

impl DeviceQualification {
    pub fn new(
        configuration: DeviceConfiguration,
        record: ArtifactId,
        authority: QualificationAuthority,
        stopping_bound_ns: Option<u64>,
    ) -> Result<Self, CoreError> {
        if stopping_bound_ns == Some(0) {
            return Err(CoreError::MissingStoppingBound);
        }
        Ok(Self {
            configuration,
            record,
            authority,
            stopping_bound_ns,
        })
    }
    pub fn configuration(&self) -> &DeviceConfiguration {
        &self.configuration
    }
    pub fn record(&self) -> &ArtifactId {
        &self.record
    }
    pub const fn stopping_bound_ns(&self) -> Option<u64> {
        self.stopping_bound_ns
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HeadlessApproval {
    configuration: DeviceConfiguration,
    record: ArtifactId,
    stopping_bound_ns: u64,
}

impl HeadlessApproval {
    /// The engine calls this only after an explicit approval by the user.
    pub fn new(qualification: &DeviceQualification) -> Result<Self, CoreError> {
        Ok(Self {
            configuration: qualification.configuration.clone(),
            record: qualification.record.clone(),
            stopping_bound_ns: qualification
                .stopping_bound_ns
                .ok_or(CoreError::MissingStoppingBound)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybackAdmission {
    pub project: ProjectId,
    pub revision: RevisionId,
    pub configuration: DeviceConfiguration,
    pub program_artifact: ArtifactId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Disarmed,
    Armed,
    Playing,
    StopRequested,
    StopUnconfirmed,
}

/// Evidence must come from a qualified driver/engine clock, never a client
/// assertion or successful transmission of a stop command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopEvidence {
    DeviceObservedStopped,
    QualifiedBoundElapsed { elapsed_ns: u64 },
}

#[derive(Clone, Debug)]
pub struct DeviceSession {
    device: DeviceId,
    session: DeviceSessionId,
    controller: Option<ClientId>,
    admission: Option<PlaybackAdmission>,
    qualification: Option<DeviceQualification>,
    headless: bool,
    physical_stop_pending: bool,
    state: DeviceState,
}

impl DeviceSession {
    pub fn new(device: DeviceId, session: DeviceSessionId) -> Self {
        Self {
            device,
            session,
            controller: None,
            admission: None,
            qualification: None,
            headless: false,
            physical_stop_pending: false,
            state: DeviceState::Disarmed,
        }
    }
    pub fn device(&self) -> &DeviceId {
        &self.device
    }
    pub fn session(&self) -> &DeviceSessionId {
        &self.session
    }
    pub fn controller(&self) -> Option<&ClientId> {
        self.controller.as_ref()
    }
    pub fn admission(&self) -> Option<&PlaybackAdmission> {
        self.admission.as_ref()
    }
    pub const fn state(&self) -> DeviceState {
        self.state
    }
    pub const fn physical_stop_pending(&self) -> bool {
        self.physical_stop_pending
    }

    pub fn arm(
        &mut self,
        controller: ClientId,
        admission: PlaybackAdmission,
        qualification: &DeviceQualification,
        headless: Option<&HeadlessApproval>,
    ) -> Result<(), CoreError> {
        if self.state != DeviceState::Disarmed {
            return Err(CoreError::InvalidTransition(
                "device must be disarmed before admission".into(),
            ));
        }
        if self.physical_stop_pending {
            return Err(CoreError::InvalidTransition(
                "prior physical stopping remains unconfirmed".into(),
            ));
        }
        if self.device != admission.configuration.device
            || qualification.configuration != admission.configuration
        {
            return Err(CoreError::MissingQualification);
        }
        if let Some(approval) = headless {
            if approval.configuration != qualification.configuration
                || approval.record != qualification.record
                || Some(approval.stopping_bound_ns) != qualification.stopping_bound_ns
            {
                return Err(CoreError::MissingStoppingBound);
            }
        }
        self.controller = Some(controller);
        self.admission = Some(admission);
        self.qualification = Some(qualification.clone());
        self.headless = headless.is_some();
        self.state = DeviceState::Armed;
        Ok(())
    }

    pub fn start(
        &mut self,
        controller: &ClientId,
        session: &DeviceSessionId,
    ) -> Result<(), CoreError> {
        self.require_controller(controller, session)?;
        if self.state != DeviceState::Armed {
            return Err(CoreError::InvalidTransition(
                "only an armed device can start".into(),
            ));
        }
        self.state = DeviceState::Playing;
        self.physical_stop_pending = true;
        Ok(())
    }

    pub fn request_stop(
        &mut self,
        controller: &ClientId,
        session: &DeviceSessionId,
    ) -> Result<(), CoreError> {
        self.require_controller(controller, session)?;
        if self.state != DeviceState::Disarmed {
            self.state = DeviceState::StopRequested;
        }
        Ok(())
    }

    /// Loss of a connected controller is distinct from authority revocation.
    /// Only separately approved headless playback may continue on disconnect.
    pub fn controller_lost(&mut self) {
        if matches!(self.state, DeviceState::Armed | DeviceState::Playing)
            && !(self.headless && self.state == DeviceState::Playing)
        {
            self.state = DeviceState::StopRequested;
        }
    }
    pub fn revoke_controller(&mut self) {
        self.headless = false;
        self.controller = None;
        if self.state != DeviceState::Disarmed {
            self.state = DeviceState::StopRequested;
        }
    }

    pub fn confirm_stopped(
        &mut self,
        session: &DeviceSessionId,
        evidence: StopEvidence,
    ) -> Result<(), CoreError> {
        if session != &self.session {
            return Err(CoreError::DeviceAuthorityMismatch);
        }
        if !matches!(
            self.state,
            DeviceState::StopRequested | DeviceState::StopUnconfirmed
        ) && !(self.state == DeviceState::Disarmed && self.physical_stop_pending)
        {
            return Err(CoreError::InvalidTransition(
                "stop evidence requires a pending or unconfirmed stop".into(),
            ));
        }
        if let StopEvidence::QualifiedBoundElapsed { elapsed_ns } = evidence {
            let bound = self
                .qualification
                .as_ref()
                .and_then(DeviceQualification::stopping_bound_ns)
                .ok_or(CoreError::MissingStoppingBound)?;
            if elapsed_ns < bound {
                return Err(CoreError::MissingStoppingBound);
            }
        }
        self.physical_stop_pending = false;
        self.clear_admission();
        Ok(())
    }
    pub fn mark_stop_unconfirmed(&mut self) -> Result<(), CoreError> {
        if self.state != DeviceState::StopRequested {
            return Err(CoreError::InvalidTransition("no stop is pending".into()));
        }
        self.state = DeviceState::StopUnconfirmed;
        Ok(())
    }

    /// Active playback cannot silently move to another project revision.
    /// This conservative first contract requires stop/rearm for every switch.
    pub fn switch_revision(
        &mut self,
        _controller: &ClientId,
        _session: &DeviceSessionId,
        _admission: PlaybackAdmission,
    ) -> Result<(), CoreError> {
        Err(CoreError::InvalidTransition(
            "revision switch requires confirmed stop and fresh admission".into(),
        ))
    }

    /// Reconnection invalidates old authorization even if device bytes match.
    /// Disarmed means no command authority, not proof that physical motion has
    /// stopped. Outstanding stop uncertainty remains set and blocks rearming.
    pub fn reconnect(&mut self, session: DeviceSessionId) -> Result<(), CoreError> {
        if session == self.session {
            return Err(CoreError::DeviceAuthorityMismatch);
        }
        self.session = session;
        self.clear_admission();
        Ok(())
    }
    fn clear_admission(&mut self) {
        self.controller = None;
        self.admission = None;
        self.qualification = None;
        self.headless = false;
        self.state = DeviceState::Disarmed;
    }
    fn require_controller(
        &self,
        controller: &ClientId,
        session: &DeviceSessionId,
    ) -> Result<(), CoreError> {
        if self.controller.as_ref() == Some(controller) && &self.session == session {
            Ok(())
        } else {
            Err(CoreError::DeviceAuthorityMismatch)
        }
    }
}
