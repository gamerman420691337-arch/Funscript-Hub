//! Authentication and request construction shared by every first-party client.
//! Only this module persists client credentials; project data remains engine-owned.

use anyhow::{bail, Context, Result};
use pulsar_protocol::*;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub trait EngineApi {
    fn execute(
        &mut self,
        command: Command,
        project: Option<ProjectId>,
        revision: Option<RevisionId>,
    ) -> Result<crate::motion::ResponseBody>;

    /// Proposes checked values through the shared upload broker; never commits.
    /// Read is separately required to hydrate the resulting candidate.
    fn upload_edit(&mut self, _project: ProjectId, _revision: RevisionId,
        _program: &MotionProgram, _label: &str) -> Result<crate::motion::CandidateSnapshot> {
        Err(ProtocolError::unsupported("edit upload on this client transport").into())
    }
}

pub struct SessionClient {
    endpoint: PathBuf,
    credentials: ClientCredentials,
    motion: crate::motion::MotionCache,
}

/// Public session identity is not an authentication secret. Do not derive
/// Debug: diagnostics must never print the bearer credential.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientCredentials {
    session: SessionId,
    auth_token: String,
}

impl ClientCredentials {
    fn validate(&self) -> Result<()> {
        if self.auth_token.is_empty() || self.auth_token.len() > 2048 {
            bail!("Invalid first-party authentication credential");
        }
        Ok(())
    }
}

fn private_new(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

struct CredentialLock(PathBuf);
impl Drop for CredentialLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn read_session(path: &Path) -> Result<Option<ClientCredentials>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
        bail!("Unsafe client credential file: {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!(
                "Client credential file must be owner-only: {}",
                path.display()
            );
        }
    }
    let credentials: ClientCredentials = serde_json::from_slice(&fs::read(path)?)
        .context("First-party credentials are invalid or use the obsolete session-only format; explicit re-pairing is required")?;
    credentials.validate()?;
    Ok(Some(credentials))
}

impl SessionClient {
    pub fn from_credentials(endpoint: &Path, session: SessionId, auth_token: String) -> Result<Self> {
        let credentials = ClientCredentials { session, auth_token };
        credentials.validate()?;
        Ok(Self { endpoint: endpoint.to_owned(), credentials, motion: crate::motion::MotionCache::default() })
    }

    pub fn connect(endpoint: &Path, bootstrap: &Path) -> Result<Self> {
        let directory = bootstrap
            .parent()
            .context("Bootstrap must live in the engine's private state directory")?;
        let credentials = directory.join("first-party.session.json");
        if let Some(credentials) = read_session(&credentials)? {
            return Ok(Self {
                endpoint: endpoint.to_owned(),
                credentials,
                motion: crate::motion::MotionCache::default(),
            });
        }
        let lock_path = directory.join("first-party.session.lock");
        let mut acquired = false;
        for _ in 0..100 {
            match private_new(&lock_path) {
                Ok(_) => {
                    acquired = true;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Some(credentials) = read_session(&credentials)? {
                        return Ok(Self {
                            endpoint: endpoint.to_owned(),
                            credentials,
                motion: crate::motion::MotionCache::default(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error.into()),
            }
        }
        if !acquired {
            bail!("First-party pairing is busy or interrupted. Check {} before explicitly removing an abandoned lock.", lock_path.display());
        }
        let _lock = CredentialLock(lock_path);
        if let Some(credentials) = read_session(&credentials)? {
            return Ok(Self {
                endpoint: endpoint.to_owned(),
                credentials,
                motion: crate::motion::MotionCache::default(),
            });
        }
        let metadata = fs::symlink_metadata(bootstrap)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
            bail!("Invalid engine bootstrap credential");
        }
        let pairing_token = fs::read_to_string(bootstrap)?.trim().to_owned();
        let request = Request::new(
            new_request_id(),
            Command::Pair {
                client_name: "Pulsar first-party clients".to_owned(),
                pairing_token,
            },
        );
        let response = call(endpoint, &request)?;
        let ResponseBody::Paired {
            session,
            auth_token,
        } = response
        else {
            bail!("Engine returned an invalid pairing response")
        };
        let credential_value = ClientCredentials {
            session,
            auth_token,
        };
        credential_value.validate()?;
        let temporary =
            directory.join(format!(".first-party.session.{}.tmp", uuid::Uuid::new_v4()));
        let outcome = (|| -> Result<()> {
            let mut file = private_new(&temporary)?;
            file.write_all(&serde_json::to_vec(&credential_value)?)?;
            file.sync_all()?;
            fs::rename(&temporary, &credentials)?;
            #[cfg(unix)]
            std::fs::File::open(directory)?.sync_all()?;
            Ok(())
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        outcome?;
        Ok(Self {
            endpoint: endpoint.to_owned(),
            credentials: credential_value,
            motion: crate::motion::MotionCache::default(),
        })
    }
}

pub fn new_request_id() -> RequestId {
    RequestId::new(uuid::Uuid::new_v4().to_string()).expect("UUID is a valid request identifier")
}

pub fn build_request(
    session: SessionId,
    auth_token: String,
    request_id: RequestId,
    command: Command,
    project: Option<ProjectId>,
    revision: Option<RevisionId>,
) -> Request {
    Request {
        version: PROTOCOL_VERSION,
        request_id,
        session: Some(session),
        auth_token: Some(auth_token),
        project,
        expected_revision: revision,
        command,
    }
}

fn call(endpoint: &Path, request: &Request) -> Result<ResponseBody> {
    let mut transport = LocalClient::connect(endpoint, Duration::from_secs(120))?;
    let response = transport.call(request)?;
    if response.version != request.version || response.request_id != request.request_id {
        bail!("Engine response identity mismatch; request outcome is unknown and was not replayed");
    }
    response.result.map_err(anyhow::Error::from)
}

impl EngineApi for SessionClient {
    fn execute(&mut self, command: Command, project: Option<ProjectId>,
        revision: Option<RevisionId>) -> Result<crate::motion::ResponseBody> {
        let expected_project = match &command {
            Command::OpenProject { project_id } => Some(project_id.clone()),
            _ => project.clone(),
        };
        let expected_candidate = match &command {
            Command::GetCandidate { candidate_id } => Some(candidate_id.clone()),
            _ => None,
        };
        let request = build_request(self.credentials.session.clone(), self.credentials.auth_token.clone(),
            new_request_id(), command, project, revision);
        let response = call(&self.endpoint, &request)?;
        let mut io = AuthenticatedMotionIo::new(&self.endpoint, &self.credentials)?;
        self.motion.hydrate(response, expected_project.as_ref(), expected_candidate.as_ref(), &mut io)
    }

    fn upload_edit(&mut self, project: ProjectId, revision: RevisionId,
        program: &MotionProgram, label: &str) -> Result<crate::motion::CandidateSnapshot> {
        let mut io = AuthenticatedMotionIo::new(&self.endpoint, &self.credentials)?;
        self.motion.upload_edit(project, revision, program, label, &mut io)
    }
}

struct AuthenticatedMotionIo<'a> {
    endpoint: &'a Path,
    credentials: &'a ClientCredentials,
    bulk_endpoint: PathBuf,
}
impl<'a> AuthenticatedMotionIo<'a> {
    fn new(endpoint: &'a Path, credentials: &'a ClientCredentials) -> Result<Self> {
        let bulk_endpoint = endpoint.parent().context("Engine endpoint has no private parent")?.join("bulk.sock");
        Ok(Self { endpoint, credentials, bulk_endpoint })
    }
}
impl crate::motion::MotionIo for AuthenticatedMotionIo<'_> {
    fn control(&mut self, command: Command, project: ProjectId, revision: Option<RevisionId>) -> Result<ResponseBody> {
        call(self.endpoint, &build_request(self.credentials.session.clone(), self.credentials.auth_token.clone(),
            new_request_id(), command, Some(project), revision))
    }

    fn bulk_endpoint(&self) -> &Path { &self.bulk_endpoint }

    fn open_bulk(&mut self, lease: &TransferLease, deadline: std::time::Instant) -> Result<Box<dyn crate::motion::BulkIo>> {
        if lease.bulk_endpoint != self.bulk_endpoint { bail!("Bulk endpoint redirect rejected before authentication"); }
        let remaining = deadline.checked_duration_since(std::time::Instant::now())
            .context("Transfer lease deadline elapsed before bulk connection")?;
        let timeout = remaining.min(Duration::from_secs(10));
        let mut bulk = BulkClient::connect(&self.bulk_endpoint, timeout)?;
        bulk.set_deadline(deadline)?;
        let actual = bulk.handshake(&BulkHandshake { version: PROTOCOL_VERSION,
            session: self.credentials.session.clone(), auth_token: self.credentials.auth_token.clone(),
            lease_id: lease.lease_id.clone(), engine_epoch: lease.engine_epoch.clone() })?;
        crate::motion::validate_handshake(lease, &actual)?;
        Ok(Box::new(bulk))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_and_gui_share_identical_authority_and_revision_envelopes() {
        let session = SessionId::new("session-1").unwrap();
        let request = RequestId::new("request-1").unwrap();
        let project = ProjectId::new("project-1").unwrap();
        let cli = build_request(
            session.clone(),
            "secret-credential".into(),
            request.clone(),
            Command::Undo,
            Some(project.clone()),
            Some(RevisionId::new(7)),
        );
        let gui = build_request(
            session,
            "secret-credential".into(),
            request,
            Command::Undo,
            Some(project),
            Some(RevisionId::new(7)),
        );
        assert_eq!(
            serde_json::to_value(cli).unwrap(),
            serde_json::to_value(gui).unwrap()
        );
    }

    #[test]
    fn session_identity_alone_is_not_a_client_credential() {
        assert!(serde_json::from_str::<ClientCredentials>("\"public-session\"").is_err());
        assert!(
            serde_json::from_str::<ClientCredentials>("{\"session\":\"public-session\"}").is_err()
        );
        assert!(ClientCredentials {
            session: SessionId::new("public-session").unwrap(),
            auth_token: String::new()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn request_authentication_is_distinct_and_redacted_in_debug() {
        let request = build_request(
            SessionId::new("public-session").unwrap(),
            "private-auth-token".into(),
            new_request_id(),
            Command::Capabilities,
            None,
            None,
        );
        assert_eq!(request.auth_token.as_deref(), Some("private-auth-token"));
        assert!(!format!("{request:?}").contains("private-auth-token"));
    }
}
