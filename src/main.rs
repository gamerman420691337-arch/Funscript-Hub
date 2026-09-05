mod funscript;
mod signal;
mod tracking;
mod video;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use funscript::{diagnose_funscript, DoctorConfig, Funscript};
use indicatif::{ProgressBar, ProgressStyle};
use signal::{detrend_and_normalize, extract_actions, integrate_flow};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracking::{
    compute_dense_flow, detect_cut_photometric, filter_centers_median, find_divergence_center,
    project_radial_motion,
};
use video::{probe_video, FrameStreamReader, StreamConfig};

#[derive(Parser)]
#[command(name = "open-fungen")]
#[command(about = "Open-source high-performance cleanroom funscript generator and editor engine", long_about = None)]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a .funscript from video using optical flow motion tracking
    Generate {
        /// Path to the video file
        video: PathBuf,

        /// Output .funscript path (defaults to <video_name>.funscript)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Target processing FPS (defaults to 30.0)
        #[arg(long, default_value_t = 30.0)]
        fps: f64,

        /// Processing frame resolution width (defaults to 256)
        #[arg(long, default_value_t = 256)]
        width: u32,

        /// Processing frame resolution height (defaults to 256)
        #[arg(long, default_value_t = 256)]
        height: u32,

        /// Detrend window in seconds (defaults to 2.0)
        #[arg(long, default_value_t = 2.0)]
        detrend_window: f64,

        /// Normalization window in seconds (defaults to 3.0)
        #[arg(long, default_value_t = 3.0)]
        norm_window: f64,

        /// Batch size in frames for tracking (defaults to 2000)
        #[arg(long, default_value_t = 2000)]
        batch_size: usize,

        /// VR mode (crops bottom-left quadrant for VR 180 stereo)
        #[arg(long, default_value_t = false)]
        vr: bool,

        /// POV mode (anchors center of motion to fixed bottom-center)
        #[arg(long, default_value_t = false)]
        pov: bool,

        /// Disable global motion compensation
        #[arg(long, default_value_t = false)]
        no_balance_global: bool,

        /// Disable keyframe reduction (records every single frame)
        #[arg(long, default_value_t = false)]
        no_keyframe_reduction: bool,

        /// Cut difference threshold (defaults to 30.0)
        #[arg(long, default_value_t = 30.0)]
        cut_diff: f32,

        /// Cut velocity magnitude threshold (defaults to 8.0)
        #[arg(long, default_value_t = 8.0)]
        cut_flow: f32,
    },

    /// Audit and lint an existing .funscript with Script Doctor
    Doctor {
        /// Path to the .funscript file
        script: PathBuf,

        /// Maximum safe speed in units/s (defaults to 450.0)
        #[arg(long, default_value_t = 450.0)]
        max_speed: f64,
    },

    /// Probe video stream details via ffprobe
    Info {
        /// Path to the video file
        video: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Generate {
            video,
            output,
            fps,
            width,
            height,
            detrend_window,
            norm_window,
            batch_size,
            vr,
            pov,
            no_balance_global,
            no_keyframe_reduction,
            cut_diff,
            cut_flow,
        } => {
            run_generate(
                &video,
                output.as_deref(),
                fps,
                width,
                height,
                detrend_window,
                norm_window,
                batch_size,
                vr,
                pov,
                !no_balance_global,
                !no_keyframe_reduction,
                cut_diff,
                cut_flow,
            )?;
        }
        Commands::Doctor { script, max_speed } => {
            run_doctor(&script, max_speed)?;
        }
        Commands::Info { video } => {
            run_info(&video)?;
        }
    }

    Ok(())
}

fn run_info(video_path: &Path) -> Result<()> {
    println!("Probing media: {}", video_path.display());
    let meta = probe_video(video_path)?;
    println!("--------------------------------------------------");
    println!("Resolution:   {}x{}", meta.width, meta.height);
    println!("Frame Rate:   {:.3} fps", meta.fps);
    println!("Duration:     {:.2} seconds", meta.duration_secs);
    println!("Total Frames: {}", meta.total_frames);
    println!("--------------------------------------------------");
    Ok(())
}

fn run_doctor(script_path: &Path, max_speed: f64) -> Result<()> {
    println!("Running Script Doctor on: {}", script_path.display());
    let script = Funscript::load(script_path)?;

    let config = DoctorConfig {
        max_safe_speed: max_speed,
        ..Default::default()
    };

    let report = diagnose_funscript(&script, &config);
    println!("==================================================");
    println!("Script Doctor Quality Audit Report");
    println!("==================================================");
    println!("Total Actions:     {}", report.total_actions);
    println!("Duration:          {:.2}s ({} ms)", report.duration_ms as f64 / 1000.0, report.duration_ms);
    println!("Position Range:    {} - {}", report.min_pos, report.max_pos);
    println!("Average Speed:     {:.1} units/s", report.avg_speed_units_per_sec);
    println!("Maximum Speed:     {:.1} units/s", report.max_speed_units_per_sec);
    println!("Speed Violations:  {}", report.speed_violations_count);
    println!("Timing Inversions: {}", report.timing_inversions_count);
    println!("Micro-jitters:     {}", report.micro_jitter_count);
    println!("--------------------------------------------------");

    if report.issues.is_empty() {
        println!("Status: PERFECT (Zero issues detected!)");
    } else {
        println!("Detected Issues (first 10 shown):");
        for issue in report.issues.iter().take(10) {
            println!(
                "  [{:?}] @ {:>7} ms: {}",
                issue.severity, issue.timestamp_ms, issue.description
            );
        }
        if report.issues.len() > 10 {
            println!("  ... and {} more issues.", report.issues.len() - 10);
        }
    }
    println!("==================================================");

    Ok(())
}

fn run_generate(
    video_path: &Path,
    output_path: Option<&Path>,
    target_fps: f64,
    width: u32,
    height: u32,
    detrend_window: f64,
    norm_window: f64,
    batch_size: usize,
    vr_mode: bool,
    pov_mode: bool,
    balance_global: bool,
    keyframe_reduction: bool,
    cut_diff_threshold: f32,
    cut_flow_threshold: f32,
) -> Result<()> {
    let t0 = Instant::now();
    let meta = probe_video(video_path).context("Failed to probe input video")?;

    let effective_output = output_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| video_path.with_extension("funscript"));

    println!("==================================================");
    println!("Open-FunGen: Video Motion Tracking Engine");
    println!("==================================================");
    println!("Input Video:   {}", video_path.display());
    println!("Output Script: {}", effective_output.display());
    println!("Original Info: {}x{}, {:.2} FPS, {:.1}s", meta.width, meta.height, meta.fps, meta.duration_secs);
    println!("Tracking Mode: {} | Grid: {}x{} | Target: {:.2} FPS",
        if vr_mode { "VR 180" } else if pov_mode { "POV" } else { "2D Divergence" },
        width, height, target_fps
    );
    println!("--------------------------------------------------");

    let stream_cfg = StreamConfig {
        target_width: width,
        target_height: height,
        target_fps,
        vr_mode,
    };

    let mut stream = FrameStreamReader::new(video_path, &stream_cfg)
        .context("Failed to initialize video decoding stream")?;

    let estimated_frames = ((meta.duration_secs * target_fps).round() as u64).max(1);
    let pb = ProgressBar::new(estimated_frames);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} frames ({eta}) - {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut all_samples: Vec<(f32, bool, i64)> = Vec::new();
    let mut frame_batch: Vec<(Vec<u8>, i64)> = Vec::with_capacity(batch_size);

    // Seed first frame
    let first = stream.next_frame()?.context("Video stream returned zero frames")?;
    let mut last_frame = first.0;

    let mut total_processed = 0u64;

    loop {
        frame_batch.clear();
        while frame_batch.len() < batch_size {
            match stream.next_frame()? {
                Some(f) => frame_batch.push(f),
                None => break,
            }
        }

        if frame_batch.is_empty() {
            break;
        }

        let batch_len = frame_batch.len();

        // 1. Process consecutive frame pairs in batch
        let mut raw_flows = Vec::with_capacity(batch_len);
        let mut raw_centers = Vec::with_capacity(batch_len);
        let mut cuts = Vec::with_capacity(batch_len);

        for (curr_frame, _curr_ts) in &frame_batch {
            let is_cut_diff = detect_cut_photometric(&last_frame, curr_frame, cut_diff_threshold);
            let flow = compute_dense_flow(&last_frame, curr_frame, width as usize, height as usize);
            let is_cut_flow = flow.mean_magnitude() > cut_flow_threshold;
            let is_cut = is_cut_diff || is_cut_flow;

            let center = if pov_mode {
                ((width / 2) as f32, (height - 1) as f32)
            } else {
                let (cx, cy, _) = find_divergence_center(&flow);
                (cx, cy)
            };

            raw_flows.push(flow);
            raw_centers.push(center);
            cuts.push(is_cut);

            last_frame = curr_frame.clone();
        }

        // 2. Filter center outliers
        let filtered_centers = filter_centers_median(&raw_centers, 6);

        // 3. Project radial motions
        for j in 0..batch_len {
            let (_, ts) = frame_batch[j];
            let radial_dot = project_radial_motion(
                &raw_flows[j],
                filtered_centers[j],
                cuts[j],
                pov_mode,
                balance_global,
            );
            all_samples.push((radial_dot, cuts[j], ts));
        }

        total_processed += batch_len as u64;
        pb.set_position(total_processed);
        pb.set_message(format!("{:.1}x speed", (total_processed as f64 / target_fps) / t0.elapsed().as_secs_f64()));
    }

    pb.finish_with_message("Tracking complete");

    if all_samples.is_empty() {
        anyhow::bail!("No frames were tracked from video");
    }

    println!("-> Integrating motion and filtering signal...");
    // 4. Numerical Integration
    let (cum_flow, timestamps) = integrate_flow(&all_samples);

    // 5. Detrending & Rolling Normalization
    let normalized = detrend_and_normalize(&cum_flow, target_fps, detrend_window, norm_window, 1000.0);

    // 6. Keyframe Extraction
    println!("-> Extracting keyframe action points...");
    let actions = extract_actions(&normalized, &timestamps, keyframe_reduction);

    let mut script = Funscript::new(actions);
    script.sanitize();

    // 7. Save output
    script.save(&effective_output)?;
    let elapsed = t0.elapsed();

    println!("==================================================");
    println!("SUCCESS: Funscript written to {}", effective_output.display());
    println!("Total Actions:  {}", script.actions.len());
    println!("Processing Time: {:.2}s ({:.1}x realtime)", elapsed.as_secs_f64(), meta.duration_secs / elapsed.as_secs_f64().max(0.01));
    println!("==================================================");

    // Run quick doctor check
    let doctor_cfg = DoctorConfig::default();
    let report = diagnose_funscript(&script, &doctor_cfg);
    println!("Script Doctor Summary:");
    println!("  Avg Speed:        {:.1} units/s", report.avg_speed_units_per_sec);
    println!("  Max Speed:        {:.1} units/s", report.max_speed_units_per_sec);
    println!("  Speed Violations: {}", report.speed_violations_count);
    println!("  Status:           {}", if report.issues.is_empty() { "EXCELLENT" } else { "VALIDATED (minor warnings)" });
    println!("==================================================");

    Ok(())
}
