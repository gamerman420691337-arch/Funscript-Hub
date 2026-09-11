//! Protocol-only modality orchestration shared by command-line entry points.
use anyhow::{bail, Result};
use pulsar_protocol::*;
use std::{path::{Path, PathBuf}, time::Duration};
use crate::{absolute_path, create_project, get_snapshot, session::EngineApi};

pub(crate) enum Input {
    Text { prompt: String },
    Image { path: PathBuf, prompt: String },
    Audio { path: PathBuf, mapping: AudioMapping, mode: AudioMode },
}

pub(crate) fn synthesize(api: &mut impl EngineApi, input: Input, output: &Path) -> Result<()> {
    let mut project = create_project(api, "Synthesized motion".into())?;
    let input = match input {
        Input::Text { prompt } => GenerationInput::Text { prompt },
        Input::Image { path, prompt } => {
            let source_version = import_source(api, &project, &path)?;
            project = get_snapshot(api, &project.project_id)?;
            GenerationInput::Image { source_version, prompt }
        }
        Input::Audio { path, mapping, mode } => {
            let source_version = import_source(api, &project, &path)?;
            project = get_snapshot(api, &project.project_id)?;
            GenerationInput::Audio { source_version, mapping, mode }
        }
    };
    eprintln!("Project: {}. Deterministic synthesis is not local AI inference.", project.project_id);
    let job = match api.execute(Command::GenerateInput { input }, Some(project.project_id.clone()), Some(project.revision))? {
        ResponseBody::Job(job) => job,
        _ => bail!("Engine returned an unexpected synthesis response"),
    };
    let candidate = await_candidate(api, &project, job)?;
    let axis = match candidate.program.tracks() { [track] => track.axis(), _ => bail!("Synthesis returned multiple or missing axes; use explicit selected-axis export") };
    commit_and_export(api, &project, candidate, output, axis)
}

fn import_source(api: &mut impl EngineApi, project: &ProjectSnapshot, path: &Path) -> Result<SourceVersionId> {
    match api.execute(Command::ImportSource { path: absolute_path(path)? }, Some(project.project_id.clone()), Some(project.revision))? {
        ResponseBody::Source { source_version } => Ok(source_version),
        _ => bail!("Engine returned an unexpected source import response"),
    }
}

pub(crate) fn import_script(api: &mut impl EngineApi, path: &Path, existing_project: Option<&str>) -> Result<()> {
    let project = match existing_project {
        Some(id) => match api.execute(Command::OpenProject { project_id: ProjectId::new(id)? }, None, None)? {
            ResponseBody::Project(project) => project,
            _ => bail!("Engine returned an unexpected project response"),
        },
        None => create_project(api, format!("Imported {}", path.file_name().unwrap_or_default().to_string_lossy()))?,
    };
    let response = api.execute(Command::ImportFunscript { path: absolute_path(path)? },
        Some(project.project_id), Some(project.revision))?;
    eprintln!("Imported script is an uncommitted candidate. Review before explicit merge/commit.");
    crate::print_response(response)
}

pub(crate) fn await_candidate(api: &mut impl EngineApi, project: &ProjectSnapshot, mut job: JobSnapshot) -> Result<CandidateSnapshot> {
    eprintln!("Job: {}. Disconnecting does not cancel engine-owned work.", job.job_id);
    let candidate_id = loop {
        if let Some(error) = &job.error { bail!("Job {}: {error}", job.job_id); }
        if let Some(candidate_id) = job.candidate_id { break candidate_id; }
        if matches!(job.state.to_ascii_lowercase().as_str(), "cancelled" | "canceled" | "failed" | "interrupted") {
            bail!("Job {} ended with state {}; project retained", job.job_id, job.state);
        }
        std::thread::sleep(Duration::from_millis(250));
        job = match api.execute(Command::JobStatus { job_id: job.job_id.clone() }, Some(project.project_id.clone()), None)? {
            ResponseBody::Job(job) => job,
            _ => bail!("Engine returned an unexpected job response"),
        };
    };
    match api.execute(Command::GetCandidate { candidate_id }, Some(project.project_id.clone()), None)? {
        ResponseBody::Candidate(candidate) => Ok(candidate),
        _ => bail!("Engine returned an unexpected candidate response"),
    }
}

pub(crate) fn commit_and_export(api: &mut impl EngineApi, project: &ProjectSnapshot, candidate: CandidateSnapshot, output: &Path, axis: Axis) -> Result<()> {
    if candidate.project_id != project.project_id || candidate.base_revision != project.revision {
        return Err(ProtocolError::new(ErrorCode::RevisionConflict, "candidate requires explicit rebase and fresh review").into());
    }
    for flag in candidate.review.iter().take(100) {
        eprintln!("REVIEW {} {:?} {:.3}..{:.3}s {:?}",
            flag.reason.as_str(), flag.axis,
            flag.range.start().as_nanos() as f64 / 1e9,
            flag.range.end().as_nanos() as f64 / 1e9, flag.evidence);
    }
    if candidate.review.len() > 100 {
        eprintln!("{} additional review flags retained on candidate {}", candidate.review.len() - 100, candidate.candidate_id);
    }
    let committed = match api.execute(Command::CommitCandidate { candidate_id: candidate.candidate_id },
        Some(project.project_id.clone()), Some(project.revision))? {
        ResponseBody::Project(project) => project,
        _ => bail!("Engine returned an unexpected candidate commit response"),
    };
    crate::print_response(api.execute(Command::ExportAxis { path: absolute_path(output)?, axis },
        Some(committed.project_id), Some(committed.revision))?)
}

pub(crate) fn parse_axis(value: &str) -> Result<Axis> {
    match value {
        "stroke" => Ok(Axis::Stroke), "sway" => Ok(Axis::Sway), "surge" => Ok(Axis::Surge),
        "roll" => Ok(Axis::Roll), "pitch" => Ok(Axis::Pitch), "yaw" | "twist" => Ok(Axis::Yaw),
        _ => Err(ProtocolError::unsupported(format!("axis '{value}'; supported: stroke,sway,surge,roll,pitch,yaw")).into()),
    }
}

pub(crate) fn parse_axes(value: &str) -> Result<Vec<Axis>> {
    let axes = value.split(',').map(parse_axis).collect::<Result<Vec<_>>>()?;
    if axes.is_empty() || axes.iter().enumerate().any(|(index, axis)| axes[..index].contains(axis)) {
        bail!("Choose nonempty distinct axes");
    }
    Ok(axes)
}

pub(crate) fn range(start: Option<f64>, end: Option<f64>) -> Result<Option<TimeRange>> {
    match (start, end) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) => {
            let convert = |value: f64| -> Result<ProjectTime> {
                let nanos = value * 1e9;
                if !nanos.is_finite() || nanos < 0.0 || nanos >= i64::MAX as f64 {
                    bail!("Project time is not a finite representable nonnegative timestamp");
                }
                Ok(ProjectTime::from_nanos(nanos.round() as i64))
            };
            Ok(Some(TimeRange::new(convert(start)?, convert(end)?)?))
        }
        _ => bail!("Both --start and --end are required for a bounded merge"),
    }
}

pub(crate) fn review(api: &mut impl EngineApi, project: &str, candidate: Option<&str>) -> Result<()> {
    crate::print_response(api.execute(Command::Diagnostics {
        candidate_id: candidate.map(CandidateId::new).transpose()?,
    }, Some(ProjectId::new(project)?), None)?)
}

pub(crate) fn merge(api: &mut impl EngineApi, project: &str, candidate: &str, axes: &str,
    start: Option<f64>, end: Option<f64>, output: Option<&Path>) -> Result<()> {
    let project = get_snapshot(api, &ProjectId::new(project)?)?;
    let selected_axes = parse_axes(axes)?;
    let export_axis = match (output, selected_axes.as_slice()) {
        (Some(_), [axis]) => Some(*axis),
        (Some(_), _) => bail!("Multi-axis merge cannot imply a single-axis export; use export-axis explicitly afterward"),
        (None, _) => None,
    };
    let merged = api.execute(Command::MergeCandidate { candidate_id: CandidateId::new(candidate)?,
        axes: selected_axes, range: range(start, end)? }, Some(project.project_id), Some(project.revision))?;
    match (merged, output) {
        (ResponseBody::Project(project), Some(path)) => crate::print_response(api.execute(
            Command::ExportAxis { path: absolute_path(path)?, axis: export_axis.expect("export axis validated before merge") }, Some(project.project_id), Some(project.revision))?),
        (response, None) => crate::print_response(response),
        _ => bail!("Engine returned an unexpected merge response"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_arguments_preserve_checked_units_and_reject_ambiguous_ranges() {
        assert!(range(Some(f64::NAN), Some(3.0)).is_err());
        assert!(range(Some(3.0), Some(1.0)).is_err());
        assert!(range(Some(0.0), None).is_err());
        let range = range(Some(0.125), Some(0.5)).unwrap().unwrap();
        assert_eq!(range.start().as_nanos(), 125_000_000);
        assert_eq!(range.end().as_nanos(), 500_000_000);
        assert!(parse_axes("stroke,stroke").is_err());
        assert!(parse_axes("suction").is_err());
        assert_eq!(parse_axis("twist").unwrap(), Axis::Yaw);
    }
}


use crate::motion::{ResponseBody, ProjectSnapshot, CandidateSnapshot};
