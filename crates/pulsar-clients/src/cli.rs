//! Public command grammar. Legacy spelling is preserved even where an engine
//! capability is not yet available; unsupported requests fail, never disappear.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "pulsar",
    version = "0.8.0",
    about = "Pulsar: local motion workstation"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Open Pulsar Desktop. GUI and CLI use the same engine interface.
    Gui,
    /// Validate a portable package and clone it into a NEW project; never overwrite.
    PackageImport {
        source: PathBuf,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long)]
        begin_request_id: Option<String>,
        #[arg(long)]
        seal_request_id: Option<String>,
    },
    /// Inspect clone-import status; a project is usable only after Completed.
    PackageImportStatus {
        operation: String,
    },
    /// Explicit engine cancellation; completed publication is never undone.
    PackageImportCancel {
        operation: String,
    },
    /// Continue a same-epoch import from the exact same package bytes.
    /// Interrupted imports require a deliberate new Start instead.
    PackageImportResume {
        operation: String,
        source: PathBuf,
        #[arg(long)]
        begin_request_id: Option<String>,
        #[arg(long)]
        seal_request_id: Option<String>,
    },

    /// Explicitly permit this local owner session to package private project data.
    AllowPackaging {
        project: String,
        #[arg(long, required = true)]
        acknowledge_unencrypted_private_data: bool,
    },
    /// Export an UNENCRYPTED portable package containing media, prompts and lineage.
    PackageExport {
        project: String,
        destination: PathBuf,
        #[arg(long, required = true)]
        acknowledge_unencrypted_private_data: bool,
        /// Explicit current revision for callers with PackageProject but no Read scope.
        #[arg(long)]
        revision: Option<u64>,
        /// Stable Start identity for recovery after a lost acknowledgement.
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Inspect a retained package operation without cancelling or downloading it.
    PackageStatus {
        project: String,
        operation: String,
    },
    /// Explicitly cancel active packaging; disconnect alone never cancels it.
    PackageCancel {
        project: String,
        operation: String,
    },
    /// Retry download of an existing Ready operation into a NEW destination.
    PackageDownload {
        project: String,
        operation: String,
        destination: PathBuf,
        /// Reuse a Begin identity only after a lost lease acknowledgement, before chunks.
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long, required = true)]
        acknowledge_unencrypted_private_data: bool,
    },
    /// Delete the retained engine package copy, not any client destination file.
    PackageRelease {
        project: String,
        operation: String,
        #[arg(long, required = true)]
        acknowledge_discarding_engine_copy: bool,
    },

    /// Export exactly one selected neutral axis from a committed project.
    ExportAxis {
        project: String,
        path: PathBuf,
        #[arg(long, default_value = "stroke")]
        axis: String,
    },
    /// Synthesize strict key=value pattern syntax; not free-form AI.
    Synthesize {
        prompt: String,
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Map full-band energy or amplitude-onset pulses, not musical beat tracking.
    AudioMap {
        video: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, default_value = "envelope")]
        mode: String,
        #[arg(long, default_value = "stroke")]
        axis: String,
        #[arg(long, default_value_t = 0.5)]
        amplitude: f64,
        #[arg(long, default_value_t = 0.25)]
        offset: f64,
    },
    /// Import a funscript as an uncommitted candidate.
    ImportScript {
        script: PathBuf,
        #[arg(long)]
        project: Option<String>,
    },
    /// Read localized evidence/protection diagnostics.
    Review {
        project: String,
        #[arg(long)]
        candidate: Option<String>,
    },
    /// Explicitly merge selected candidate axes and project-time range.
    MergeCandidate {
        project: String,
        candidate: String,
        #[arg(long, default_value = "stroke")]
        axes: String,
        #[arg(long)]
        start: Option<f64>,
        #[arg(long)]
        end: Option<f64>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Repair a script using the engine's qualified repair capability.
    Fix {
        script: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, default_value_t = 450.0)]
        max_speed: f64,
        #[arg(long, default_value_t = 20)]
        jitter_window: i64,
        #[arg(long, default_value_t = 2)]
        jitter_threshold: i32,
    },
    /// Generate, review engine completion, commit, and export a stroke program.
    Generate {
        video: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        model: Option<PathBuf>,
        /// Explicit class count for the RGB/NCHW YOLO channel-major model ABI.
        #[arg(long)]
        model_class_count: Option<u32>,
        #[arg(long, default_value_t = 0.35)]
        conf: f32,
        #[arg(long, default_value_t = 30.0)]
        fps: f64,
        #[arg(long)]
        width: Option<u32>,
        #[arg(long)]
        height: Option<u32>,
        #[arg(long, default_value_t = 2.0)]
        detrend_window: f64,
        #[arg(long, default_value_t = 3.0)]
        norm_window: f64,
        #[arg(long, default_value_t = 2000)]
        batch_size: usize,
        #[arg(long, default_value_t = false)]
        vr: bool,
        #[arg(long, default_value_t = false)]
        pov: bool,
        #[arg(long, default_value_t = false)]
        no_balance_global: bool,
        #[arg(long, default_value_t = false)]
        no_keyframe_reduction: bool,
        #[arg(long, default_value_t = 30.0)]
        cut_diff: f32,
        #[arg(long, default_value_t = 8.0)]
        cut_flow: f32,
        #[arg(long, default_value_t = false)]
        multi_axis: bool,
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long)]
        cuts: Option<PathBuf>,
    },
    /// Run the engine's script diagnostics.
    Doctor {
        script: PathBuf,
        #[arg(long, default_value_t = 450.0)]
        max_speed: f64,
    },
    /// Probe media through an isolated engine worker.
    Info {
        video: PathBuf,
    },
    #[command(alias = "audit")]
    Scan {
        folder: PathBuf,
        #[arg(short, long, default_value_t = true)]
        recursive: bool,
        #[arg(long, default_value_t = false)]
        multi_axis: bool,
        #[arg(long, default_value_t = false)]
        missing_only: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long, default_value_t = false)]
        csv: bool,
        #[arg(long, default_value_t = false)]
        generate: bool,
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        overwrite: bool,
    },
    Batch {
        folder: PathBuf,
        #[arg(short, long, default_value_t = true)]
        recursive: bool,
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        multi_axis: bool,
        #[arg(long, default_value_t = false)]
        overwrite: bool,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    AudioSynth {
        video: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, default_value = "beats")]
        mode: String,
        #[arg(long, default_value = "bass")]
        band: String,
        #[arg(long, default_value = "stroke")]
        axis: String,
        #[arg(long, default_value_t = 0.35)]
        threshold: f32,
        #[arg(long, default_value_t = false)]
        bundle: bool,
    },
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },
    Bench,
    Completions {
        shell: clap_complete::Shell,
    },
    Play {
        video: PathBuf,
        #[arg(short, long)]
        script: Option<PathBuf>,
        #[arg(long)]
        serial: Option<String>,
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        #[arg(long)]
        handy: Option<String>,
        #[arg(long)]
        buttplug: Option<String>,
    },
    Stash {
        #[command(subcommand)]
        command: StashCommands,
    },
    /// Print the engine's actual capabilities and qualification status.
    Capabilities,
    /// Create or open a durable engine-owned project.
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
}

#[derive(Debug, Subcommand)]
pub enum ModelCommands {
    List,
    Download {
        #[arg(default_value = "default")]
        model: String,
    },
    Status,
}

#[derive(Debug, Subcommand)]
pub enum StashCommands {
    #[command(alias = "query")]
    ListMissing {
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long, default_value = "http://localhost:9999/graphql")]
        url: String,
        #[arg(long)]
        api_key: Option<String>,
    },
    AutoGenerate {
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long, default_value = "http://localhost:9999/graphql")]
        url: String,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        multi_axis: bool,
        #[arg(long)]
        tag: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommands {
    Create { name: String },
    Open { project_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_aliases_remain_parseable() {
        assert!(matches!(
            Cli::try_parse_from(["pulsar", "audit", "/media"])
                .unwrap()
                .command,
            Some(Commands::Scan { .. })
        ));
        assert!(matches!(
            Cli::try_parse_from(["pulsar", "stash", "query"])
                .unwrap()
                .command,
            Some(Commands::Stash {
                command: StashCommands::ListMissing { .. }
            })
        ));
    }

    #[test]
    fn complete_legacy_generate_grammar_is_preserved() {
        Cli::try_parse_from([
            "pulsar",
            "generate",
            "video.mp4",
            "-o",
            "out.funscript",
            "--model",
            "model.onnx",
            "--conf",
            "0.5",
            "--fps",
            "24",
            "--width",
            "256",
            "--height",
            "256",
            "--detrend-window",
            "2",
            "--norm-window",
            "3",
            "--batch-size",
            "20",
            "--vr",
            "--pov",
            "--no-balance-global",
            "--no-keyframe-reduction",
            "--cut-diff",
            "30",
            "--cut-flow",
            "8",
            "--multi-axis",
            "--profile",
            "dense",
            "--cuts",
            "cuts.json",
        ])
        .unwrap();
    }
}
