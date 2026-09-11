//! P1a private-engine closure KAT. Not portable export/import qualification.
//!
//! All source and motion inputs are authored synthetic fixtures. No device is
//! actuated. The separately ignored worker case requires explicit host
//! qualification and never manufactures worker output or execution authority.

use super::*;
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT_CLOSURE_ID: AtomicU64 = AtomicU64::new(1);
const PROJECT_NAME: &str = "P1a independent closure";
const FOREIGN_MARKER: &str = "OTHER_PROJECT_MUST_NOT_LEAK_8f79b37a";
const IMPORT_BYTES: &[u8] =
    br#"{"version":"1.0","actions":[{"at":0,"pos":35},{"at":100,"pos":45},{"at":200,"pos":40},{"at":300,"pos":50}]}"#;
const MEDIA_BYTES: &[u8] = b"P6\n2 2\n255\n\x00\x00\x00\xff\xff\xff\xff\x00\x00\x00\xff\x00";
const LEGACY_EMPTY_BYTES: &[u8] = b"\n { \"tracks\" : [ ] } \t\n";

fn fresh_request() -> RequestId {
    RequestId::new(format!("closure-{}-{}", std::process::id(),
        NEXT_CLOSURE_ID.fetch_add(1, Ordering::Relaxed))).unwrap()
}
fn sha(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

struct Fixture {
    root: PathBuf,
    worker: PathBuf,
    engine: Option<Arc<Engine>>,
    session: SessionId,
    auth_token: String,
    completed: bool,
}
impl Fixture {
    fn new(worker: Option<PathBuf>) -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("pulsar-package-closure-{}-{nonce}", std::process::id()));
        fs::create_dir(&root)?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        let worker = match worker {
            Some(path) => path.canonicalize().context("explicit package worker executable is unavailable")?,
            None => {
                let path = root.join("non-executable-worker-placeholder");
                fs::write(&path, b"No job may execute through this ordinary closure fixture.")?;
                path
            }
        };
        let engine = Engine::open(EngineConfig::new(root.join("state"), worker.clone()))?;
        let pair = Request::new(fresh_request(), Command::Pair {
            client_name: "P1a closure fixture owner".into(),
            pairing_token: engine.token.clone(),
        });
        let (session, auth_token) = match engine.handle(pair).result? {
            ResponseBody::Paired { session, auth_token } => (session, auth_token),
            _ => bail!("real engine pairing did not return credentials"),
        };
        Ok(Self {root,worker,engine:Some(engine),session,auth_token,completed:false})
    }
    fn engine(&self) -> &Arc<Engine> { self.engine.as_ref().unwrap() }
    fn request(&self, command: Command, project: Option<&ProjectId>, revision: Option<u64>) -> Request {
        let mut request = Request::new(fresh_request(), command)
            .with_session(self.session.clone()).with_auth_token(self.auth_token.clone());
        request.project = project.cloned();
        request.expected_revision = revision.map(RevisionId::new);
        request
    }
    fn call(&self, command: Command, project: Option<&ProjectId>, revision: Option<u64>) -> Result<ResponseBody> {
        Ok(self.engine().handle(self.request(command,project,revision)).result?)
    }
    fn create(&self, name: &str) -> Result<ProjectId> {
        let project = match self.call(Command::CreateProject {name:name.into()},None,None)? {
            ResponseBody::Project(project) => project,
            _ => bail!("real create did not return project"),
        };
        ensure!(project.revision == RevisionId::new(0),"project must start at revision zero");
        Ok(project.project_id)
    }
    fn current(&self, project: &ProjectId) -> Result<RevisionId> {
        match self.call(Command::GetSnapshot,Some(project),None)? {
            ResponseBody::Project(snapshot) => Ok(snapshot.revision),
            _ => bail!("snapshot did not return project"),
        }
    }
    fn upload(&self, project: &ProjectId, revision: u64, values: &MotionProgram, label: &str) -> Result<CandidateId> {
        let bytes = serde_json::to_vec(&pulsar_core::EditValuesProgram::from_program_values(values)?)?;
        let lease = match self.call(Command::BeginEditUpload {
            byte_len:bytes.len() as u64,sha256:sha(&bytes),label:label.into(),
        },Some(project),Some(revision))? {
            ResponseBody::Transfer(lease) => lease,
            _ => bail!("real edit admission did not return transfer"),
        };
        let handshake = BulkHandshake {
            version:PROTOCOL_VERSION,session:self.session.clone(),auth_token:self.auth_token.clone(),
            lease_id:lease.lease_id.clone(),engine_epoch:lease.engine_epoch,
        };
        ensure!(self.engine().bulk_upload(&handshake,0,&bytes)? == bytes.len() as u64,
            "small fixture upload was not fully acknowledged");
        let candidate = match self.call(Command::FinishEditUpload {lease_id:lease.lease_id},
            Some(project),None)? {
            ResponseBody::Candidate(candidate) => candidate,
            _ => bail!("real edit finalization did not return candidate"),
        };
        ensure!(candidate.base_revision == RevisionId::new(revision),"candidate base changed");
        ensure!(self.current(project)? == RevisionId::new(revision),"finalization committed a proposal");
        Ok(candidate.candidate_id)
    }
    fn commit(&self, project: &ProjectId, base: u64, candidate: &CandidateId) -> Result<()> {
        let response = self.call(Command::CommitCandidate {candidate_id:candidate.clone()},Some(project),Some(base))?;
        match response {
            ResponseBody::Project(snapshot) => ensure!(snapshot.revision == RevisionId::new(base+1),"unexpected commit revision"),
            _ => bail!("commit did not return project"),
        }
        Ok(())
    }
    fn capture(&self, project: &ProjectId, revision: u64) -> Result<project_packages::ProjectCapturePlan> {
        Ok(project_packages::capture(self.engine(),&self.session,&self.auth_token,project,RevisionId::new(revision))?)
    }
    fn reopen(&mut self) -> Result<()> {
        drop(self.engine.take());
        self.engine=Some(Engine::open(EngineConfig::new(self.root.join("state"),self.worker.clone()))?);
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.engine.take());
        if self.completed && !std::thread::panicking() { let _=fs::remove_dir_all(&self.root); }
        else { eprintln!("P1a closure fixture evidence retained at {}",self.root.display()); }
    }
}

fn motion(branch: u8) -> MotionProgram {
    let stroke = match branch { 1=>[35,45,40,52],2|3=>[35,45,40,48],_=>[35,45,40,50] };
    let sway = if branch==3 { [20,22,21,25] } else { [20,22,21,23] };
    MotionProgram::new([(Axis::Stroke,stroke),(Axis::Sway,sway)].into_iter().map(|(axis,values)| {
        MotionTrack::new(axis,values.into_iter().enumerate().map(|(i,pos)| {
            MotionAction::new(ProjectTime::from_nanos(i as i64*100_000_000),
                NormalizedPosition::new(f64::from(pos)/100.0).unwrap(),EvidenceKind::Observed).unwrap()
        }).collect()).unwrap()
    }).collect()).unwrap()
}
fn protected() -> Vec<ProtectedRegion> {
    vec![ProtectedRegion{axis:Some(Axis::Stroke),
        range:TimeRange::new(ProjectTime::from_nanos(100_000_000),ProjectTime::from_nanos(150_000_000)).unwrap()}]
}
struct Expected {
    project: ProjectId,
    media: SourceVersionId,
    imported: CandidateId,
    authored: CandidateId,
    abandoned: CandidateId,
    branch: CandidateId,
    uncommitted: CandidateId,
    export_hash: String,
}
fn build_project(f: &Fixture) -> Result<Expected> {
    let project=f.create(PROJECT_NAME)?;
    let media_path=f.root.join("authored-catalog-fixture.ppm");
    fs::write(&media_path,MEDIA_BYTES)?;
    let media=match f.call(Command::ImportSource{path:media_path.clone()},Some(&project),None)? {
        ResponseBody::Source{source_version}=>source_version,_=>bail!("source import did not return identity"),
    };
    f.call(Command::PinSource{source_version:media.clone(),pinned:true},Some(&project),None)?;
    // The mutable locator is deliberately no longer a valid source. Package
    // resolution must use the admitted immutable snapshot, not reread it.
    fs::write(&media_path,b"Changed original must never replace admitted source bytes.")?;
    let script_path=f.root.join("authored-import.funscript");
    fs::write(&script_path,IMPORT_BYTES)?;
    let imported=match f.call(Command::ImportFunscript{path:script_path},Some(&project),Some(0))? {
        ResponseBody::Candidate(candidate)=>candidate.candidate_id,_=>bail!("script import did not return candidate"),
    };
    f.commit(&project,0,&imported)?;
    let authored=f.upload(&project,1,&motion(0),"two-axis authored proposal")?;
    f.commit(&project,1,&authored)?;
    let abandoned=f.upload(&project,2,&motion(1),"later abandoned branch")?;
    f.commit(&project,2,&abandoned)?;
    f.call(Command::Undo,Some(&project),Some(3))?;
    ensure!(f.current(&project)?==RevisionId::new(4),"undo must create fresh revision");
    let branch=f.upload(&project,4,&motion(2),"replacement branch")?;
    f.commit(&project,4,&branch)?;
    f.call(Command::SetProtectedRegions{regions:protected()},Some(&project),Some(5))?;
    ensure!(f.current(&project)?==RevisionId::new(6),"protection must create fresh revision");
    let uncommitted=f.upload(&project,6,&motion(3),"retained uncommitted proposal")?;
    let export_path=f.root.join("explicit-stroke-export.funscript");
    match f.call(Command::ExportAxis{path:export_path.clone(),axis:Axis::Stroke},Some(&project),Some(6))? {
        ResponseBody::AxisExported{path,revision,axis}=>ensure!(path==export_path &&
            revision==RevisionId::new(6) && axis==Axis::Stroke,"axis export receipt binding changed"),
        _=>bail!("explicit axis export did not produce receipt"),
    }
    let export_bytes=fs::read(&export_path)?;
    let exported:serde_json::Value=serde_json::from_slice(&export_bytes)?;
    let actions=exported["actions"].as_array().context("export lacks standard actions")?;
    ensure!(actions.len()==4,"neutral export lost or invented actions");
    for (index,action) in actions.iter().enumerate() {
        ensure!(action["at"].as_u64()==Some(index as u64*100) &&
            action["pos"].as_u64()==Some([35,45,40,48][index]),"independent neutral export oracle changed");
    }
    let export_hash=sha(&export_bytes);

    let other=f.create(FOREIGN_MARKER)?;
    let secret=f.root.join("other-project-private.ppm");
    fs::write(&secret,FOREIGN_MARKER.as_bytes())?;
    f.call(Command::ImportSource{path:secret},Some(&other),None)?;
    Ok(Expected{project,media,imported,authored,abandoned,branch,uncommitted,export_hash})
}
fn object_bytes(plan: &project_packages::ProjectCapturePlan, hash: &str) -> Result<Vec<u8>> {
    let descriptor=plan.manifest.objects.iter().find(|o|o.sha256==hash).context("required descriptor absent")?;
    // Every object in these fixtures is deliberately small. This bound is a
    // fixture oracle, not a substitute for production package admission.
    ensure!(descriptor.byte_len<1024*1024,"fixture unexpectedly contains a large object");
    let mut bytes=Vec::new();
    plan.open_object(descriptor)?.take(descriptor.byte_len+1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64==descriptor.byte_len,"object length disagrees with descriptor");
    ensure!(sha(&bytes)==descriptor.sha256,"object content disagrees with digest");
    Ok(bytes)
}
fn check_program(bytes: &[u8], shape: u8) -> Result<()> {
    let program:MotionProgram=serde_json::from_slice(bytes)?;
    if shape==10 {ensure!(program.tracks().is_empty(),"initial empty program changed");return Ok(());}
    let expected_axes=if shape==11 {vec![Axis::Stroke]} else {vec![Axis::Stroke,Axis::Sway]};
    ensure!(program.tracks().len()==expected_axes.len(),"motion axis lost or invented");
    let stroke=match shape {1=>[35,45,40,52],2|3=>[35,45,40,48],_=>[35,45,40,50]};
    let sway=if shape==3 {[20,22,21,25]}else{[20,22,21,23]};
    for axis in expected_axes {
        let track=program.track(axis).context("expected axis absent")?;
        ensure!(track.actions().len()==4 && track.gaps().is_empty(),"motion actions/gaps changed");
        for (i,action) in track.actions().iter().enumerate() {
            let value=if axis==Axis::Stroke {stroke[i]} else {sway[i]};
            ensure!(action.time().as_nanos()==i as i64*100_000_000,"exact timeline changed");
            ensure!(action.position().value().to_bits()==(f64::from(value)/100.0).to_bits(),"motion value changed");
        }
    }
    Ok(())
}
fn check_closure(m: &PortableProjectManifest,e: &Expected,legacy: bool) -> Result<()> {
    ensure!(m.origin_project_id==e.project && m.head.name==PROJECT_NAME,"wrong project closure");
    ensure!(m.captured_revision==RevisionId::new(6) && m.head.revision==RevisionId::new(6),"wrong captured head");
    ensure!(m.captured_event_cursor>0,"event cursor omitted");
    ensure!(m.head.history_cursor==4,"retained history cursor changed");
    ensure!(m.revisions.iter().map(|r|r.revision).collect::<Vec<_>>() ==
        (0..=6).map(RevisionId::new).collect::<Vec<_>>(),"persisted/abandoned revision omitted");
    ensure!(m.edit_states.iter().map(|s|s.position).collect::<Vec<_>>()==vec![0,1,2,3,4],"retained history stack changed");
    ensure!(m.edit_states.iter().map(|s|s.source_revision).collect::<Vec<_>>() ==
        [0,1,2,5,6].into_iter().map(|r|Some(RevisionId::new(r))).collect::<Vec<_>>(),"retained restoration associations changed");
    ensure!(m.revision_lineage.len()==7,"revision lineage row omitted");
    let undo=m.revision_lineage.iter().find(|r|r.revision==RevisionId::new(4)).context("undo edge omitted")?;
    ensure!(undo.parent_revision==Some(RevisionId::new(3)) &&
        undo.restored_from_revision==Some(RevisionId::new(2)) &&
        undo.restored_history_position==Some(2),"undo restoration edge changed");
    ensure!(undo.operation=="undo","undo operation lost");
    for (revision,candidate) in [(1,&e.imported),(2,&e.authored),(3,&e.abandoned),(5,&e.branch)] {
        let row=m.revision_lineage.iter().find(|r|r.revision==RevisionId::new(revision)).context("candidate revision edge omitted")?;
        ensure!(row.candidate.as_ref()==Some(candidate),"candidate association changed");
    }
    let head_lineage=m.revision_lineage.iter().find(|r|r.revision==RevisionId::new(6)).context("head lineage absent")?;
    if legacy {
        ensure!(head_lineage.review.unknown && head_lineage.review.conservative,"explicit unknown/conservative ancestry lost");
        ensure!(head_lineage.parent_revision.is_none() && head_lineage.restored_from_revision.is_none(),"legacy ancestry invented");
    }
    ensure!(serde_json::to_value(&m.head.protected)?==serde_json::to_value(protected())?,"head protection changed");
    for revision in &m.revisions {
        ensure!(revision.protected.len()==usize::from(revision.revision==RevisionId::new(6)),"historical protection changed");
    }
    ensure!(m.candidates.len()==5 && m.authored_lineage.len()==4,"candidate or authored lineage omitted");
    for (id,base,committed) in [
        (&e.imported,0,Some(1)),(&e.authored,1,Some(2)),(&e.abandoned,2,Some(3)),
        (&e.branch,4,Some(5)),(&e.uncommitted,6,None)
    ] {
        let candidate=m.candidates.iter().find(|c|&c.candidate_id==id).context("durable candidate omitted")?;
        ensure!(candidate.base_revision==RevisionId::new(base),"candidate base changed");
        ensure!(candidate.committed_revision==committed.map(RevisionId::new),"candidate commit state changed");
        ensure!(candidate.job_origin.is_none(),"authored/imported proposal invented a worker job");
    }
    let imported=m.candidates.iter().find(|c|c.candidate_id==e.imported).unwrap();
    ensure!(!imported.review.is_empty(),"imported conservative review disappeared");
    // These receipt counts describe contiguous ancestry intervals, not knots:
    // the first edit adds one whole axis; later edits replace one endpoint
    // interval while retaining a stroke/sway prefix and the other whole axis.
    for (id,authored,inherited) in [(&e.authored,1,1),(&e.abandoned,1,2),(&e.branch,1,2),(&e.uncommitted,1,2)] {
        let row=m.authored_lineage.iter().find(|r|&r.candidate_id==id).context("authored receipt association omitted")?;
        ensure!(row.authored_count==authored && row.inherited_count==inherited,"authored/inherited accounting changed for {}: actual {}/{}, expected {}/{}",id.as_str(),row.authored_count,row.inherited_count,authored,inherited);
        ensure!(row.receipt.byte_len>0 && row.input.byte_len>0,"required compact evidence omitted");
        let candidate=m.candidates.iter().find(|c|&c.candidate_id==id).unwrap();
        ensure!(candidate.lineage.iter().any(|identity|identity.artifact_id==row.receipt.artifact_id &&
            identity.sha256==row.receipt.sha256 && identity.byte_len==row.receipt.byte_len),
            "candidate compact lineage lost its exact authored receipt");
        ensure!(!row.review.flags.is_empty(),"inherited imported-motion review lost");
    }
    ensure!(m.sources.len()==2,"source catalog incomplete or other-project source leaked");
    let media=m.sources.iter().find(|s|s.source_version==e.media).context("media source identity omitted")?;
    ensure!(media.kind==SourceKind::Media && media.identity.sha256==sha(MEDIA_BYTES),"media kind or immutable content changed");
    ensure!(media.pinned && !media.evicted,"media pin/availability changed");
    let script=m.sources.iter().find(|s|s.kind==SourceKind::Funscript).context("funscript kind omitted")?;
    ensure!(script.identity.sha256==sha(IMPORT_BYTES),"original script bytes changed");
    ensure!(m.generated_origins.is_empty(),"ordinary fixture invented generation origin");
    ensure!(m.export_receipts.len()==1,"project export receipt omitted or another project leaked");
    let export=&m.export_receipts[0];
    ensure!(export.revision==RevisionId::new(6) && export.axis==Axis::Stroke &&
        export.sha256==e.export_hash && matches!(export.state,PackageExportState::Complete),
        "inert export receipt binding/state changed");
    ensure!(m.legacy_cells.len()==usize::from(legacy),"recovery cell omitted or fabricated");
    if legacy {
        let cell=&m.legacy_cells[0];
        ensure!(cell.owner==PackageLegacyOwner::Revisions && cell.row_key==format!("{}:0",e.project.as_str()),"legacy owner changed");
        ensure!(cell.object.sha256==sha(LEGACY_EMPTY_BYTES) && cell.object.byte_len==LEGACY_EMPTY_BYTES.len() as u64,"legacy original bytes changed");
    }
    ensure!(matches!(m.completeness.source_bytes,PackageSourceBytes::NeedsMaterialization),"P1a claimed export readiness");
    m.validate()?;
    Ok(())
}
fn check_objects(plan:&project_packages::ProjectCapturePlan,e:&Expected)->Result<()> {
    for object in &plan.manifest.objects {let _=object_bytes(plan,&object.sha256)?;}
    for revision in &plan.manifest.revisions {
        let n=serde_json::to_value(revision.revision)?.as_u64().unwrap();
        let shape=match n {0=>10,1=>11,2|4=>0,3=>1,5|6=>2,_=>bail!("unexpected revision")};
        check_program(&object_bytes(plan,&revision.motion.sha256)?,shape)?;
    }
    for (id,shape) in [(&e.imported,11),(&e.authored,0),(&e.abandoned,1),(&e.branch,2),(&e.uncommitted,3)] {
        let row=plan.manifest.candidates.iter().find(|c|&c.candidate_id==id).unwrap();
        check_program(&object_bytes(plan,&row.motion.sha256)?,shape)?;
    }
    ensure!(object_bytes(plan,&sha(MEDIA_BYTES))?==MEDIA_BYTES,"mutable original substituted");
    ensure!(object_bytes(plan,&sha(IMPORT_BYTES))?==IMPORT_BYTES,"script snapshot changed");
    Ok(())
}

#[test]
fn project_closure_exact_durable_history_candidates_sources_and_protection() -> Result<()> {
    let mut f=Fixture::new(None)?;
    let expected=build_project(&f)?;
    let before=f.capture(&expected.project,6)?;
    check_closure(&before.manifest,&expected,false)?;
    check_objects(&before,&expected)?;
    let canonical=before.manifest.canonical_bytes()?;
    ensure!(before.manifest_sha256==sha(&canonical),"capture digest does not bind exact manifest bytes");
    let visible=String::from_utf8(canonical.clone())?;
    ensure!(!visible.contains(FOREIGN_MARKER) && !visible.contains(&f.auth_token) &&
        !visible.contains(&f.engine().token),"another project or bearer credential leaked");
    let mut missing_edge=before.manifest.clone();
    missing_edge.revision_lineage.retain(|r|r.revision!=RevisionId::new(4));
    missing_edge.counts=missing_edge.record_counts()?;
    ensure!(check_closure(&missing_edge,&expected,false).is_err(),"KAT did not detect missing restoration edge");
    let mut missing_candidate=before.manifest.clone();
    missing_candidate.candidates.retain(|c|c.candidate_id!=expected.uncommitted);
    missing_candidate.counts=missing_candidate.record_counts()?;
    ensure!(check_closure(&missing_candidate,&expected,false).is_err(),"KAT did not detect missing uncommitted proposal");
    drop(before);
    f.reopen()?;
    let after=f.capture(&expected.project,6)?;
    check_closure(&after.manifest,&expected,false)?;
    check_objects(&after,&expected)?;
    ensure!(after.manifest.canonical_bytes()?==canonical,"durable reopen changed captured logical identity");
    f.completed=true;
    Ok(())
}

#[test]
fn project_closure_preserves_exact_legacy_cells_and_explicit_unknown_review() -> Result<()> {
    let mut f=Fixture::new(None)?;
    let expected=build_project(&f)?;
    // Explicit legacy migration fixture only. The inline cell is semantically
    // the existing empty R0 program; no motion or positive ancestry is forged.
    {
        let db=f.engine().db.lock().unwrap();
        ensure!(db.execute("UPDATE revisions SET program=?1 WHERE project=?2 AND revision=0",
            rusqlite::params![std::str::from_utf8(LEGACY_EMPTY_BYTES)?,expected.project.as_str()])?==1);
        ensure!(db.execute("DELETE FROM revision_lineage WHERE project=?1 AND revision=6",
            [expected.project.as_str()])?==1);
    }
    f.reopen()?;
    let plan=f.capture(&expected.project,6)?;
    check_closure(&plan.manifest,&expected,true)?;
    check_objects(&plan,&expected)?;
    ensure!(object_bytes(&plan,&sha(LEGACY_EMPTY_BYTES))?==LEGACY_EMPTY_BYTES,"recovery bytes were re-encoded");
    let mut unknown_lost=plan.manifest.clone();
    unknown_lost.revision_lineage.iter_mut().find(|r|r.revision==RevisionId::new(6)).unwrap().review.unknown=false;
    ensure!(check_closure(&unknown_lost,&expected,true).is_err(),"KAT did not detect lost unknown ancestry");
    let mut recovery_lost=plan.manifest.clone();
    recovery_lost.legacy_cells.clear();recovery_lost.counts=recovery_lost.record_counts()?;
    ensure!(check_closure(&recovery_lost,&expected,true).is_err(),"KAT did not detect lost recovery cell");
    f.completed=true;
    Ok(())
}

#[test]
fn project_closure_rejects_missing_required_edit_receipt_without_mutating_head() -> Result<()> {
    let mut f=Fixture::new(None)?;
    let expected=build_project(&f)?;
    let plan=f.capture(&expected.project,6)?;
    let receipt=plan.manifest.authored_lineage.iter().find(|r|r.candidate_id==expected.uncommitted)
        .context("uncommitted authored receipt absent")?.receipt.clone();
    ensure!(receipt.artifact_id.as_str().starts_with("edit-receipt."),"unexpected receipt locator namespace");
    let mut missing=plan.manifest.clone();
    missing.objects.retain(|object|object.sha256!=receipt.sha256);
    missing.counts=missing.record_counts()?;
    ensure!(missing.validate().is_err(),"required receipt descriptor silently became optional");
    drop(plan);
    let path=f.root.join("state").join("edit-receipts").join(&receipt.sha256);
    fs::remove_file(&path).context("remove only legitimate fixture-owned receipt")?;
    let error=match f.capture(&expected.project,6) {Ok(_)=>bail!("capture accepted missing required receipt"),Err(error)=>error};
    ensure!(error.to_string().contains(&receipt.sha256),"missing-evidence failure omitted content identity");
    ensure!(f.current(&expected.project)?==RevisionId::new(6),"failed closure changed project head");
    f.completed=true;
    Ok(())
}

#[test]
fn project_closure_requires_distinct_package_scope_and_expected_revision() -> Result<()> {
    let mut f=Fixture::new(None)?;
    let expected=build_project(&f)?;
    let (reader,secret)=match f.engine().handle(Request::new(fresh_request(),Command::Pair {
        client_name:"neutral-only export reader".into(),pairing_token:f.engine().token.clone(),
    })).result? {
        ResponseBody::Paired{session,auth_token}=>(session,auth_token),_=>bail!("reader pairing failed"),
    };
    f.call(Command::Grant{session:reader.clone(),project_id:expected.project.clone(),scopes:vec![Scope::Read,Scope::Export]},None,None)?;
    let error=match project_packages::capture(f.engine(),&reader,&secret,&expected.project,RevisionId::new(6)) {
        Ok(_)=>bail!("neutral export/read granted private package authority"),Err(error)=>error,
    };
    ensure!(error.code==ErrorCode::Forbidden,"wrong package-authority rejection");
    let error=match project_packages::capture(f.engine(),&f.session,&f.auth_token,&expected.project,RevisionId::new(5)) {
        Ok(_)=>bail!("stale revision captured current source/candidate closure"),Err(error)=>error,
    };
    ensure!(error.code==ErrorCode::RevisionConflict,"stale package revision not rejected");
    ensure!(f.current(&expected.project)?==RevisionId::new(6),"rejected capture mutated project");
    f.completed=true;
    Ok(())
}

#[test]
#[ignore = "requires PULSAR_PACKAGE_TEST_WORKER and qualified Linux worker confinement"]
fn project_closure_captures_actual_generated_input_and_inert_job_origin() -> Result<()> {
    let worker=std::env::var_os("PULSAR_PACKAGE_TEST_WORKER")
        .context("explicit qualification requires PULSAR_PACKAGE_TEST_WORKER")?;
    let mut f=Fixture::new(Some(PathBuf::from(worker)))?;
    let project=f.create("P1a actual constrained text worker")?;
    let input=GenerationInput::Text{prompt:"pattern=hold axis=stroke duration=2s frequency=0hz amplitude=0 offset=0.42".into()};
    let source_bytes=serde_json::to_vec(&input)?;
    let initial=match f.call(Command::GenerateInput{input},Some(&project),Some(0))? {
        ResponseBody::Job(job)=>job,_=>bail!("generation admission did not return real job"),
    };
    let deadline=Instant::now()+Duration::from_secs(60);
    let completed=loop {
        let job=match f.call(Command::JobStatus{job_id:initial.job_id.clone()},Some(&project),None)? {
            ResponseBody::Job(job)=>job,_=>bail!("job query did not return real job"),
        };
        match job.state.as_str() {
            "completed"=>break job,
            "failed"|"cancelled"|"interrupted"=>bail!("real worker did not qualify: {} {:?}",job.state,job.error),
            _=>{}
        }
        ensure!(Instant::now()<deadline,"real constrained worker completion timed out");
        std::thread::sleep(Duration::from_millis(20));
    };
    let candidate=completed.candidate_id.context("completed worker supplied no candidate")?;
    let plan=f.capture(&project,0)?;
    let manifest=&plan.manifest;
    ensure!(manifest.sources.len()==1 && manifest.generated_origins.len()==1,"generated closure lost source/attempt origin");
    ensure!(manifest.candidates.len()==1 && manifest.authored_lineage.is_empty(),"worker was relabelled as an authored upload");
    let source=&manifest.sources[0];
    ensure!(source.kind==SourceKind::GenerationInput && source.identity.sha256==sha(&source_bytes),"typed generation input identity changed");
    ensure!(object_bytes(&plan,&source.identity.sha256)?==source_bytes,"typed source bytes were rewritten");
    let origin=&manifest.generated_origins[0];
    ensure!(origin.job_id==initial.job_id && origin.attempt_id==initial.attempt_id &&
        origin.base_revision==RevisionId::new(0) && origin.source_version==source.source_version && origin.state=="completed",
        "historical job/attempt/source association changed");
    let raw_program=origin.program.as_ref().context("successful worker raw program identity missing")?;
    let receipt=origin.receipt.as_ref().context("successful worker receipt missing")?;
    let raw_motion:MotionProgram=serde_json::from_slice(&object_bytes(&plan,&raw_program.sha256)?)?;
    let receipt_json:serde_json::Value=serde_json::from_slice(&object_bytes(&plan,&receipt.sha256)?)?;
    ensure!(receipt_json["qualification"]=="unqualified" &&
        receipt_json["program"]["sha256"]==raw_program.sha256,
        "original receipt qualification or raw-program identity changed");
    let mut raw_identity_lost=manifest.clone();
    raw_identity_lost.generated_origins[0].program=None;
    ensure!(raw_identity_lost.validate().is_err(),"successful origin silently lost required original program");
    let candidate_record=&manifest.candidates[0];
    ensure!(candidate_record.candidate_id==candidate && candidate_record.job_origin.as_ref()==Some(&initial.job_id) &&
        candidate_record.committed_revision.is_none(),"uncommitted generated origin changed");
    ensure!(!candidate_record.review.is_empty(),"unqualified constrained generation review disappeared");
    let motion:MotionProgram=serde_json::from_slice(&object_bytes(&plan,&candidate_record.motion.sha256)?)?;
    for program in [&motion,&raw_motion] {
        let track=program.track(Axis::Stroke).context("generated stroke missing")?;
        ensure!(program.tracks().len()==1 && track.actions().len()==2,"constrained hold oracle axes/action count changed");
        for (index,action) in track.actions().iter().enumerate() {
            ensure!(action.time().as_nanos()==index as i64*2_000_000_000 &&
                action.position().value().to_bits()==0.42f64.to_bits() &&
                action.evidence()==EvidenceKind::Synthesized,"constrained hold oracle changed");
        }
    }
    let historical:serde_json::Value=serde_json::from_slice(&object_bytes(&plan,&origin.manifest.sha256)?)?;
    ensure!(historical["job_id"]==serde_json::to_value(&initial.job_id)? &&
        historical["attempt_id"]==serde_json::to_value(&initial.attempt_id)?,
        "exact historical worker manifest lost origin identities");
    let canonical=manifest.canonical_bytes()?;
    ensure!(f.current(&project)?==RevisionId::new(0),"closure capture committed generated proposal");
    drop(plan);
    f.reopen()?;
    let reopened=f.capture(&project,0)?;
    ensure!(reopened.manifest.canonical_bytes()?==canonical,"reopen altered inert generated origin");
    ensure!(f.current(&project)?==RevisionId::new(0),"reopen activated generated candidate");
    f.completed=true;
    Ok(())
}


#[test]
fn project_closure_keeps_redo_restoration_edges_after_branch_truncation() -> Result<()> {
    let mut f=Fixture::new(None)?;
    let project=f.create("P1a explicit redo branch")?;
    let path=f.root.join("redo-origin.funscript");
    fs::write(&path,IMPORT_BYTES)?;
    let imported=match f.call(Command::ImportFunscript{path},Some(&project),Some(0))? {
        ResponseBody::Candidate(candidate)=>candidate.candidate_id,_=>bail!("import did not return candidate"),
    };
    f.commit(&project,0,&imported)?;
    let a=f.upload(&project,1,&motion(0),"redo baseline")?;
    f.commit(&project,1,&a)?;
    let b=f.upload(&project,2,&motion(1),"branch later abandoned")?;
    f.commit(&project,2,&b)?;
    for (base,command) in [(3,Command::Undo),(4,Command::Redo),(5,Command::Undo)] {
        f.call(command,Some(&project),Some(base))?;
        ensure!(f.current(&project)?==RevisionId::new(base+1),"history restoration reused an old revision");
    }
    let c=f.upload(&project,6,&motion(2),"branch truncates old redo state")?;
    f.commit(&project,6,&c)?;
    let captured=f.capture(&project,7)?;
    let m=&captured.manifest;
    ensure!(m.revisions.iter().map(|r|r.revision).collect::<Vec<_>>()==
        (0..=7).map(RevisionId::new).collect::<Vec<_>>(),"redo or abandoned revision omitted");
    ensure!(m.head.history_cursor==3 && m.edit_states.len()==4,"new branch did not retain exact history stack");
    ensure!(m.edit_states[3].source_revision==Some(RevisionId::new(7)),"truncated redo slot was not replaced by branch");
    for (revision,parent,restored,position,operation,shape) in [
        (4,3,2,2,"undo",0),(5,4,3,3,"redo",1),(6,5,2,2,"undo",0)
    ] {
        let row=m.revision_lineage.iter().find(|r|r.revision==RevisionId::new(revision)).context("restoration edge omitted")?;
        ensure!(row.parent_revision==Some(RevisionId::new(parent)) &&
            row.restored_from_revision==Some(RevisionId::new(restored)) &&
            row.restored_history_position==Some(position) && row.operation==operation,
            "explicit restoration edge lost or inferred from current slot");
        let motion=&m.revisions.iter().find(|r|r.revision==RevisionId::new(revision)).unwrap().motion;
        check_program(&object_bytes(&captured,&motion.sha256)?,shape)?;
    }
    ensure!(m.candidates.iter().any(|candidate|candidate.candidate_id==b &&
        candidate.committed_revision==Some(RevisionId::new(3))),"abandoned branch candidate was dropped");
    let canonical=m.canonical_bytes()?;
    drop(captured);f.reopen()?;
    let reopened=f.capture(&project,7)?;
    ensure!(reopened.manifest.canonical_bytes()?==canonical,"redo restoration changed across reopen");
    f.completed=true;
    Ok(())
}
