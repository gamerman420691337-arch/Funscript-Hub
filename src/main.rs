#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod audio;
mod batch;
mod funscript;
mod gui;
mod kinematics;
mod neural;
mod plugin;
mod signal;
mod stash;
mod sync;
mod tracking;
mod video;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use funscript::{diagnose_funscript, AxisChannel, DoctorConfig, Funscript, MultiAxisScript};
use indicatif::{ProgressBar, ProgressStyle};
use neural::tracker::AnatomicalTracker;
use neural::NeuralDetector;
use signal::{detrend_and_normalize, extract_actions, extract_actions_full_range, integrate_flow};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracking::{
    compute_dense_flow, detect_cut_photometric, filter_centers_median, find_divergence_center,
    project_adaptive_motion, project_companion_motion,
};
use video::{probe_video, rgb_to_gray, FrameStreamReader, StreamConfig};

#[derive(Parser)]
#[command(name = "pulsar")]
#[command(about = "⚡ Pulsar: Neural Kinematic Workstation & Funscript Generator", long_about = None)]
#[command(version = "0.8.0")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the interactive GUI timeline editor and generator
    Gui,

    /// Automatically repair an existing .funscript (clamp speed violations, remove micro-jitters, sort/dedup)
    Fix {
        /// Path to the .funscript file
        script: PathBuf,

        /// Path to save repaired .funscript (defaults to <name>.fixed.funscript)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Maximum safe hardware speed in units/s (defaults to 450.0)
        #[arg(long, default_value_t = 450.0)]
        max_speed: f64,

        /// Micro-jitter time threshold in ms (defaults to 20)
        #[arg(long, default_value_t = 20)]
        jitter_window: i64,

        /// Micro-jitter position delta threshold (defaults to 2)
        #[arg(long, default_value_t = 2)]
        jitter_threshold: i32,
    },

    /// Generate a .funscript from video using optical flow motion tracking or neural ONNX detection
    Generate {
        /// Path to the video file
        video: PathBuf,

        /// Output .funscript path (defaults to <video_name>.funscript)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Path to ONNX neural tracking model (e.g. YOLOv12n / FunGen model)
        #[arg(long)]
        model: Option<PathBuf>,

        /// Neural detection confidence threshold (defaults to 0.35)
        #[arg(long, default_value_t = 0.35)]
        conf: f32,

        /// Target processing FPS (defaults to 30.0)
        #[arg(long, default_value_t = 30.0)]
        fps: f64,

        /// Processing frame resolution width (defaults to 256 for flow, 640 for neural)
        #[arg(long)]
        width: Option<u32>,

        /// Processing frame resolution height (defaults to 256 for flow, 640 for neural)
        #[arg(long)]
        height: Option<u32>,

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

        /// Generate full 6-DOF companion bundle (.surge, .sway, .pitch, .twist, .suction)
        #[arg(long, default_value_t = false)]
        multi_axis: bool,

        /// Adaptive vision-to-motion profile: economy, specialist, default, dense, geometry
        #[arg(long, default_value = "default")]
        profile: String,
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

    /// Batch process an entire folder of videos in headless queue mode
    Batch {
        /// Directory containing video files
        folder: PathBuf,

        /// Recursively scan subdirectories for videos
        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        /// Path to ONNX neural tracking model
        #[arg(long)]
        model: Option<PathBuf>,

        /// Generate full 6-DOF companion bundle
        #[arg(long, default_value_t = false)]
        multi_axis: bool,

        /// Overwrite existing .funscript files
        #[arg(long, default_value_t = false)]
        overwrite: bool,
    },

    /// Synthesize haptic funscripts directly from audio frequency bands (Sub-Bass, Mid, High)
    AudioSynth {
        /// Path to the video or audio file
        video: PathBuf,

        /// Output .funscript path (defaults to <name>.funscript)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Coupling mode: beats (turnaround pulses), envelope (continuous dynamics), or vibe (8Hz oscillation)
        #[arg(long, default_value = "beats")]
        mode: String,

        /// Frequency band for primary channel: bass, mid, high, or full
        #[arg(long, default_value = "bass")]
        band: String,

        /// Target hardware channel: stroke, surge, sway, pitch, roll, twist, or suction
        #[arg(long, default_value = "stroke")]
        axis: String,

        /// Sensitivity threshold (0.05 to 0.90)
        #[arg(long, default_value_t = 0.35)]
        threshold: f32,

        /// Generate full multi-band companion bundle (Bass->Stroke, Bass->Surge, Mid->Twist, High->Suction)
        #[arg(long, default_value_t = false)]
        bundle: bool,
    },

    /// Stash Media Server GraphQL integration and automation
    Stash {
        #[command(subcommand)]
        command: StashCommands,
    },
}

#[derive(Subcommand)]
enum StashCommands {
    /// Query Stash server for scenes missing .funscripts
    #[command(alias = "query")]
    ListMissing {
        /// Maximum number of scenes to query
        #[arg(long, default_value_t = 50)]
        limit: u32,

        /// Stash GraphQL endpoint URL
        #[arg(long, default_value = "http://localhost:9999/graphql")]
        url: String,

        /// Optional Stash API key
        #[arg(long)]
        api_key: Option<String>,
    },

    /// Automatically generate .funscripts for all Stash scenes missing scripts
    AutoGenerate {
        /// Maximum number of scenes to process
        #[arg(long, default_value_t = 50)]
        limit: u32,

        /// Stash GraphQL endpoint URL
        #[arg(long, default_value = "http://localhost:9999/graphql")]
        url: String,

        /// Optional Stash API key
        #[arg(long)]
        api_key: Option<String>,

        /// Path to ONNX neural tracking model
        #[arg(long)]
        model: Option<PathBuf>,

        /// Generate full 6-DOF companion bundle
        #[arg(long, default_value_t = false)]
        multi_axis: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Commands::Gui) => {
            println!("Launching Pulsar Desktop Application...");
            gui::run_gui().map_err(|e| anyhow::anyhow!("GUI execution error: {e}"))?;
        }
        Some(Commands::Fix {
            script,
            output,
            max_speed,
            jitter_window,
            jitter_threshold,
        }) => {
            run_fix(&script, output.as_deref(), max_speed, jitter_window, jitter_threshold)?;
        }
        Some(Commands::Generate {
            video,
            output,
            model,
            conf,
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
            multi_axis,
            profile,
        }) => {
            let eff_width = width.unwrap_or(if model.is_some() { 640 } else { 256 });
            let eff_height = height.unwrap_or(if model.is_some() { 640 } else { 256 });

            run_generate(
                &video,
                output.as_deref(),
                model.as_deref(),
                conf,
                fps,
                eff_width,
                eff_height,
                detrend_window,
                norm_window,
                batch_size,
                vr,
                pov,
                !no_balance_global,
                !no_keyframe_reduction,
                cut_diff,
                cut_flow,
                multi_axis,
                &profile,
            )?;
        }
        Some(Commands::Doctor { script, max_speed }) => {
            run_doctor(&script, max_speed)?;
        }
        Some(Commands::Info { video }) => {
            run_info(&video)?;
        }
        Some(Commands::Batch {
            folder,
            recursive,
            model,
            multi_axis,
            overwrite,
        }) => {
            run_batch(&folder, recursive, model.as_deref(), multi_axis, overwrite)?;
        }
        Some(Commands::AudioSynth {
            video,
            output,
            mode,
            band,
            axis,
            threshold,
            bundle,
        }) => {
            run_audio_synth(
                &video,
                output.as_deref(),
                &mode,
                &band,
                &axis,
                threshold,
                bundle,
            )?;
        }
        Some(Commands::Stash { command }) => match command {
            StashCommands::ListMissing { limit, url, api_key } => {
                run_stash_list_missing(&url, api_key.as_deref(), limit)?;
            }
            StashCommands::AutoGenerate {
                limit,
                url,
                api_key,
                model,
                multi_axis,
            } => {
                run_stash_auto_generate(&url, api_key.as_deref(), limit, model.as_deref(), multi_axis)?;
            }
        },
    }

    Ok(())
}

fn run_fix(
    script_path: &Path,
    output_path: Option<&Path>,
    max_speed: f64,
    jitter_window: i64,
    jitter_threshold: i32,
) -> Result<()> {
    println!("Loading funscript for repair: {}", script_path.display());
    let mut script = Funscript::load(script_path)?;
    let initial_count = script.actions.len();

    script.sanitize();
    let sanitized_count = script.actions.len();
    let removed_jitters = script.remove_micro_jitters(jitter_window, jitter_threshold);
    let fixed_speeds = script.fix_speed_violations(max_speed);

    let effective_output = match output_path {
        Some(p) => p.to_path_buf(),
        None => {
            let stem = script_path.file_stem().unwrap_or_default().to_string_lossy();
            let parent = script_path.parent().unwrap_or_else(|| Path::new("."));
            parent.join(format!("{}.fixed.funscript", stem))
        }
    };

    script.save(&effective_output)?;

    println!("==================================================");
    println!("Script Doctor Repair Complete");
    println!("==================================================");
    println!("Original Actions: {}", initial_count);
    println!("Deduplicated:     {}", initial_count - sanitized_count);
    println!("Jitters Removed:  {}", removed_jitters);
    println!("Speeds Clamped:   {}", fixed_speeds);
    println!("Final Actions:    {}", script.actions.len());
    println!("Output Saved To:  {}", effective_output.display());
    println!("==================================================");

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

#[allow(clippy::too_many_arguments)]
fn run_generate(
    video_path: &Path,
    output_path: Option<&Path>,
    model_path: Option<&Path>,
    conf_threshold: f32,
    target_fps: f64,
    width: u32,
    height: u32,
    _detrend_window: f64,
    _norm_window: f64,
    _batch_size: usize,
    vr_mode: bool,
    pov_mode: bool,
    balance_global: bool,
    keyframe_reduction: bool,
    cut_diff_threshold: f32,
    cut_flow_threshold: f32,
    multi_axis: bool,
    profile_str: &str,
) -> Result<()> {
    let t0 = Instant::now();
    let meta = probe_video(video_path).context("Failed to probe input video")?;

    let effective_output = output_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| video_path.with_extension("funscript"));

    let is_neural = model_path.is_some();
    let adaptive_profile = crate::neural::pipeline::AdaptiveProfile::parse_str(profile_str)
        .unwrap_or(crate::neural::pipeline::AdaptiveProfile::GenericDefault);

    println!("==================================================");
    println!("Pulsar: Video Motion Tracking Engine");
    println!("==================================================");
    println!("Input Video:   {}", video_path.display());
    println!("Output Script: {}", effective_output.display());
    println!("Original Info: {}x{}, {:.2} FPS, {:.1}s", meta.width, meta.height, meta.fps, meta.duration_secs);
    println!("Tracking Mode: {} | Grid: {}x{} | Target: {:.2} FPS",
        if is_neural { "Adaptive Fast/Slow Vision-to-Motion" } else if vr_mode { "VR 180 (Optical Flow)" } else if pov_mode { "POV (Optical Flow)" } else { "2D Divergence (Optical Flow)" },
        width, height, target_fps
    );
    println!("ML Profile:    {} (cadence k={})", adaptive_profile.name(), adaptive_profile.refresh_cadence());
    let backend = if crate::neural::is_cuda_supported() {
        "NVIDIA CUDA (Hardware Accelerated)"
    } else {
        "CPU (Multi-Threaded SIMD)"
    };
    println!("Compute Backend: {}", backend);
    if let Some(m) = model_path {
        println!("ONNX Model:    {} (conf: {:.2})", m.display(), conf_threshold);
    }
    if multi_axis {
        println!("Multi-Axis:    ENABLED (Generating L0, L1, L2, R1, R0, R2, V0)");
    }
    println!("--------------------------------------------------");

    let mut neural_detector = if let Some(m) = model_path {
        Some(NeuralDetector::new(m, conf_threshold)?)
    } else {
        None
    };

    let mut anatomical_tracker = if is_neural {
        let mut trk = AnatomicalTracker::new();
        trk.router.config.refresh_cadence_k = adaptive_profile.refresh_cadence();
        Some(trk)
    } else {
        None
    };

    let effective_fps = if (target_fps - 30.0).abs() < 1e-4 && meta.fps > 5.0 && meta.fps <= 60.0 {
        meta.fps
    } else {
        target_fps
    };

    let stream_cfg = StreamConfig {
        target_width: width,
        target_height: height,
        target_fps: effective_fps,
        vr_mode,
        is_rgb: is_neural,
    };

    let mut stream = FrameStreamReader::new(video_path, &stream_cfg)
        .context("Failed to initialize video decoding stream")?;

    let estimated_frames = ((meta.duration_secs * effective_fps).round() as u64).max(1);
    let pb = ProgressBar::new(estimated_frames);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} frames ({eta}) - {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut all_samples: Vec<(f32, bool, i64)> = Vec::new();
    let mut surge_samples: Vec<(f32, bool, i64)> = Vec::new();
    let mut sway_samples: Vec<(f32, bool, i64)> = Vec::new();
    let mut pitch_samples: Vec<(f32, bool, i64)> = Vec::new();
    let mut roll_samples: Vec<(f32, bool, i64)> = Vec::new();
    let mut neural_poses: Vec<(crate::neural::Pose3D, i64)> = Vec::new();

    let mut flow_ctx = crate::tracking::FlowScratchContext::new(0, 0);

    // Helper for 2x2 average downscale to 256x256 (grayscale input)
    fn downscale_to_256(input: &[u8], width: usize, height: usize) -> Vec<u8> {
        let mut out = vec![0; 256 * 256];
        let dx = width as f32 / 256.0;
        let dy = height as f32 / 256.0;
        for y in 0..256 {
            for x in 0..256 {
                let sx = (x as f32 * dx) as usize;
                let sy = (y as f32 * dy) as usize;
                let sx2 = (sx + 1).min(width - 1);
                let sy2 = (sy + 1).min(height - 1);
                
                let i0 = sy * width + sx;
                let i1 = sy * width + sx2;
                let i2 = sy2 * width + sx;
                let i3 = sy2 * width + sx2;
                
                let v = (input[i0] as u16 + input[i1] as u16 + input[i2] as u16 + input[i3] as u16) / 4;
                out[y * 256 + x] = v as u8;
            }
        }
        out
    }

    /// Fused RGB → grayscale + spatial downscale to 256×256 in a single pass.
    /// Eliminates the intermediate full-resolution grayscale allocation.
    fn rgb_downscale_to_gray_256(rgb: &[u8], width: usize, height: usize) -> Vec<u8> {
        let mut out = vec![0u8; 256 * 256];
        let dx = width as f32 / 256.0;
        let dy = height as f32 / 256.0;
        for y in 0..256 {
            let sy = (y as f32 * dy) as usize;
            let sy2 = (sy + 1).min(height - 1);
            for x in 0..256 {
                let sx = (x as f32 * dx) as usize;
                let sx2 = (sx + 1).min(width - 1);

                // Sample 4 RGB pixels, convert each to luma (Rec.601 integer),
                // then average the 4 luma values
                let luma = |px: usize, py: usize| -> u16 {
                    let base = (py * width + px) * 3;
                    let r = rgb[base] as u32;
                    let g = rgb[base + 1] as u32;
                    let b = rgb[base + 2] as u32;
                    ((r * 77 + g * 150 + b * 29) >> 8) as u16
                };

                let v = (luma(sx, sy) + luma(sx2, sy) + luma(sx, sy2) + luma(sx2, sy2)) / 4;
                out[y * 256 + x] = v as u8;
            }
        }
        out
    }

    struct BufferedFrame {
        raw: Option<Vec<u8>>,
        ts: i64,
        flow: crate::tracking::FlowField,
        center: (f32, f32),
        is_cut: bool,
    }

    let mut window: Vec<BufferedFrame> = Vec::with_capacity(14);
    
    // Process a frame at target_idx in the window
    let emit_frame = |target_idx: usize,
                      window: &[BufferedFrame],
                      all_samples: &mut Vec<(f32, bool, i64)>,
                      surge_samples: &mut Vec<(f32, bool, i64)>,
                      sway_samples: &mut Vec<(f32, bool, i64)>,
                      pitch_samples: &mut Vec<(f32, bool, i64)>,
                      roll_samples: &mut Vec<(f32, bool, i64)>,
                      neural_poses: &mut Vec<_>,
                      detector: &mut Option<NeuralDetector>,
                      tracker: &mut Option<AnatomicalTracker>|
     -> Result<()> {
        let centers: Vec<_> = window.iter().map(|w| w.center).collect();
        let filtered = filter_centers_median(&centers, 6);
        let target_center = filtered[target_idx];

        let frame = &window[target_idx];
        let dot = project_adaptive_motion(&frame.flow, target_center, frame.is_cut, pov_mode, balance_global);
        all_samples.push((dot, frame.is_cut, frame.ts));

        if multi_axis {
            let (surge, sway, pitch, roll) = project_companion_motion(
                &frame.flow,
                target_center,
                frame.is_cut,
                pov_mode,
                balance_global,
            );
            surge_samples.push((surge, frame.is_cut, frame.ts));
            sway_samples.push((sway, frame.is_cut, frame.ts));
            pitch_samples.push((pitch, frame.is_cut, frame.ts));
            roll_samples.push((roll, frame.is_cut, frame.ts));
        }

        if let (Some(det), Some(trk)) = (detector, tracker) {
            if let Some(raw_frame) = &frame.raw {
                let detections = det.detect(raw_frame, width as usize, height as usize)?;
                let (pose, _, _branch, _obs) = trk.update_pose_adaptive(
                    Some(&detections),
                    None,
                    Some(&frame.flow),
                    false,
                    frame.ts,
                    frame.is_cut,
                );
                neural_poses.push((pose, frame.ts));
            }
        }
        Ok(())
    };

    // Seed first frame
    let (mut last_raw, _first_ts) = stream.next_frame()?.context("Video stream returned zero frames")?;
    let mut last_gray = if stream_cfg.is_rgb && !is_neural { Some(rgb_to_gray(&last_raw)) } else { None };
    let mut last_flow_gray = if is_neural {
        if stream_cfg.is_rgb {
            // Fused: RGB → gray + downscale in one pass (no intermediate W×H gray buffer)
            Some(rgb_downscale_to_gray_256(&last_raw, width as usize, height as usize))
        } else {
            Some(downscale_to_256(&last_raw, width as usize, height as usize))
        }
    } else {
        None
    };

    let mut total_processed = 0u64;

    while let Some((mut curr_raw, curr_ts)) = stream.next_frame()? {
        let curr_gray = if stream_cfg.is_rgb && !is_neural { Some(rgb_to_gray(&curr_raw)) } else { None };
        let curr_flow_gray = if is_neural {
            if stream_cfg.is_rgb {
                Some(rgb_downscale_to_gray_256(&curr_raw, width as usize, height as usize))
            } else {
                Some(downscale_to_256(&curr_raw, width as usize, height as usize))
            }
        } else {
            None
        };

        let flow_w = if is_neural { 256 } else { width as usize };
        let flow_h = if is_neural { 256 } else { height as usize };

        let (prev_ref, curr_ref) = if is_neural {
            (last_flow_gray.as_ref().unwrap().as_slice(), curr_flow_gray.as_ref().unwrap().as_slice())
        } else if stream_cfg.is_rgb {
            (last_gray.as_ref().unwrap().as_slice(), curr_gray.as_ref().unwrap().as_slice())
        } else {
            (last_raw.as_slice(), curr_raw.as_slice())
        };

        let is_cut_diff = detect_cut_photometric(prev_ref, curr_ref, cut_diff_threshold);
        // NOTE: clone() copies 524KB per frame. A future optimization is to have
        // compute_dense_flow write into a caller-owned rotating FlowField pool,
        // but replacing ctx.field would force re-allocation on the next resize().
        let flow = compute_dense_flow(prev_ref, curr_ref, flow_w, flow_h, &mut flow_ctx).clone();
        let is_cut = is_cut_diff && (flow.mean_magnitude() > cut_flow_threshold);

        let center = if pov_mode {
            ((flow_w / 2) as f32, (flow_h - 1) as f32)
        } else {
            let (cx, cy, _) = find_divergence_center(&flow);
            (cx, cy)
        };

        let frame_raw = if is_neural {
            std::mem::take(&mut curr_raw)
        } else {
            Vec::new()
        };

        window.push(BufferedFrame {
            raw: if is_neural { Some(frame_raw) } else { None },
            ts: curr_ts,
            flow,
            center,
            is_cut,
        });

        // Advance state
        if stream_cfg.is_rgb {
            last_gray = curr_gray;
        } else {
            std::mem::swap(&mut last_raw, &mut curr_raw);
        }
        if is_neural {
            last_flow_gray = curr_flow_gray;
        }

        if window.len() == 13 {
            emit_frame(6, &window, &mut all_samples, &mut surge_samples, &mut sway_samples, &mut pitch_samples, &mut roll_samples, &mut neural_poses, &mut neural_detector, &mut anatomical_tracker)?;
            window.remove(0);
            total_processed += 1;
        } else if window.len() >= 7 {
            let emit_idx = window.len() - 7;
            emit_frame(emit_idx, &window, &mut all_samples, &mut surge_samples, &mut sway_samples, &mut pitch_samples, &mut roll_samples, &mut neural_poses, &mut neural_detector, &mut anatomical_tracker)?;
            total_processed += 1;
        }

        if total_processed % 10 == 0 {
            pb.set_position(total_processed);
            pb.set_message(format!("{:.1}x speed", (total_processed as f64 / effective_fps) / t0.elapsed().as_secs_f64()));
        }
    }

    // Flush remaining
    let remaining = window.len();
    if remaining > 0 {
        let start_idx = if remaining >= 7 { remaining - 6 } else { 0 };
        for i in start_idx..remaining {
            emit_frame(i, &window, &mut all_samples, &mut surge_samples, &mut sway_samples, &mut pitch_samples, &mut roll_samples, &mut neural_poses, &mut neural_detector, &mut anatomical_tracker)?;
            total_processed += 1;
        }
        pb.set_position(total_processed);
    }

    pb.finish_with_message("Tracking complete");

    if all_samples.is_empty() {
        anyhow::bail!("No frames were tracked from video");
    }

    let mut bundle = MultiAxisScript::new();

    let neural_valid = if is_neural && !neural_poses.is_empty() {
        let stroke_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.stroke).collect();
        let min_s = stroke_values.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max_s = stroke_values.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
        (max_s - min_s) > 5.0
    } else {
        false
    };

    if neural_valid {
        println!("-> Processing neural 6-DOF pose observations (SAM 2 / Co-Tracker temporal point tracking)...");
        let timestamps: Vec<i64> = neural_poses.iter().map(|(_, ts)| *ts).collect();
        let stroke_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.stroke).collect();

        let mut stroke_script = Funscript::new(extract_actions_full_range(&stroke_values, &timestamps, 3.5, true));
        stroke_script.sanitize();
        bundle.channels.insert(AxisChannel::Stroke, stroke_script);

        if multi_axis {
            println!("-> Synthesizing full 6-DOF companion channels...");
            let surge_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.surge).collect();
            let mut surge_script = Funscript::new(extract_actions(&surge_values, &timestamps, keyframe_reduction));
            surge_script.sanitize();
            bundle.channels.insert(AxisChannel::Surge, surge_script);

            let sway_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.sway).collect();
            let mut sway_script = Funscript::new(extract_actions(&sway_values, &timestamps, keyframe_reduction));
            sway_script.sanitize();
            bundle.channels.insert(AxisChannel::Sway, sway_script);

            let pitch_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.pitch).collect();
            let mut pitch_script = Funscript::new(extract_actions(&pitch_values, &timestamps, keyframe_reduction));
            pitch_script.sanitize();
            bundle.channels.insert(AxisChannel::Pitch, pitch_script);

            let roll_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.roll).collect();
            let mut roll_script = Funscript::new(extract_actions(&roll_values, &timestamps, keyframe_reduction));
            roll_script.sanitize();
            bundle.channels.insert(AxisChannel::Roll, roll_script);

            let twist_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.twist).collect();
            let mut twist_script = Funscript::new(extract_actions(&twist_values, &timestamps, keyframe_reduction));
            twist_script.sanitize();
            bundle.channels.insert(AxisChannel::Twist, twist_script);

            let suction_values: Vec<f32> = neural_poses.iter().map(|(p, _)| p.suction).collect();
            let mut suction_script = Funscript::new(extract_actions(&suction_values, &timestamps, keyframe_reduction));
            suction_script.sanitize();
            bundle.channels.insert(AxisChannel::Suction, suction_script);
        }
    } else {
        if is_neural {
            println!("-> Neural detection found insufficient anatomical landmark motion in this video; seamlessly falling back to high-fidelity optical motion tracking...");
        }
        println!("-> Integrating optical motion and filtering signal...");
        let (cum_flow, timestamps) = integrate_flow(&all_samples);
        println!("-> Extracting full-range keyframe action points (0 to 100)...");
        let mut stroke_script = Funscript::new(extract_actions_full_range(
            &cum_flow,
            &timestamps,
            0.8,
            true,
        ));
        stroke_script.sanitize();
        bundle.channels.insert(AxisChannel::Stroke, stroke_script);

        if multi_axis {
            println!("-> Synthesizing optical 6-DOF companion channels from motion geometry...");
            let (surge_cum, _) = integrate_flow(&surge_samples);
            let surge_norm = detrend_and_normalize(&surge_cum, effective_fps, 2.0, 3.0, 50.0);
            let mut surge_script = Funscript::new(extract_actions(&surge_norm, &timestamps, keyframe_reduction));
            surge_script.sanitize();
            bundle.channels.insert(AxisChannel::Surge, surge_script);

            let (sway_cum, _) = integrate_flow(&sway_samples);
            let sway_norm = detrend_and_normalize(&sway_cum, effective_fps, 2.0, 3.0, 50.0);
            let mut sway_script = Funscript::new(extract_actions(&sway_norm, &timestamps, keyframe_reduction));
            sway_script.sanitize();
            bundle.channels.insert(AxisChannel::Sway, sway_script);

            let (pitch_cum, _) = integrate_flow(&pitch_samples);
            let pitch_norm = detrend_and_normalize(&pitch_cum, effective_fps, 2.0, 3.0, 50.0);
            let mut pitch_script = Funscript::new(extract_actions(&pitch_norm, &timestamps, keyframe_reduction));
            pitch_script.sanitize();
            bundle.channels.insert(AxisChannel::Pitch, pitch_script);

            let (roll_cum, _) = integrate_flow(&roll_samples);
            let roll_norm = detrend_and_normalize(&roll_cum, effective_fps, 2.0, 3.0, 50.0);
            let mut roll_script = Funscript::new(extract_actions(&roll_norm, &timestamps, keyframe_reduction));
            roll_script.sanitize();
            bundle.channels.insert(AxisChannel::Roll, roll_script);
        }
    };

    if multi_axis {
        println!("-> Saving multi-axis companion bundle...");
        bundle.save_bundle(&effective_output)?;
    } else if let Some(stroke) = bundle.channels.get(&AxisChannel::Stroke) {
        stroke.save(&effective_output)?;
    }

    let elapsed = t0.elapsed();
    let primary_script = bundle.channels.get(&AxisChannel::Stroke).unwrap();

    println!("==================================================");
    if multi_axis {
        println!("SUCCESS: Multi-Axis 6-DOF Bundle saved for {}", effective_output.display());
        println!("Active Channels: {}", bundle.channels.len());
    } else {
        println!("SUCCESS: Funscript written to {}", effective_output.display());
    }
    println!("Stroke Actions:  {}", primary_script.actions.len());
    println!("Processing Time: {:.2}s ({:.1}x realtime)", elapsed.as_secs_f64(), meta.duration_secs / elapsed.as_secs_f64().max(0.01));
    println!("==================================================");

    // Run quick doctor check on primary stroke
    let doctor_cfg = DoctorConfig::default();
    let report = diagnose_funscript(primary_script, &doctor_cfg);
    println!("Script Doctor Summary (Stroke L0):");
    println!("  Avg Speed:        {:.1} units/s", report.avg_speed_units_per_sec);
    println!("  Max Speed:        {:.1} units/s", report.max_speed_units_per_sec);
    println!("  Speed Violations: {}", report.speed_violations_count);
    println!("  Status:           {}", if report.issues.is_empty() { "EXCELLENT" } else { "VALIDATED (minor warnings)" });
    println!("==================================================");

    if let Some(trk) = &anatomical_tracker {
        let stats = trk.router.execution_stats();
        println!("Adaptive ML Routing Statistics (Pareto Optimization):");
        println!("  Fast Path (TAPNext++/Flow): {:.1}%", stats.fast_percent);
        println!("  Specialist Refresh:         {:.1}%", stats.refresh_percent);
        println!("  Dense Repair (CoWTracker):  {:.1}%", stats.dense_repair_percent);
        println!("  Semantic Reacquisition:     {:.1}%", stats.reacquisition_percent);
        println!("  Unresolved Intervals:       {:.1}%", stats.unresolved_percent);
        println!("==================================================");
    }

    Ok(())
}

fn run_batch(
    folder: &Path,
    recursive: bool,
    model: Option<&Path>,
    multi_axis: bool,
    overwrite: bool,
) -> Result<()> {
    use batch::queue::{BatchJobConfig, BatchQueue};

    println!("==================================================");
    println!("Pulsar: Headless Batch Processing Queue");
    println!("==================================================");
    println!("Target Directory: {}", folder.display());
    println!("Recursive Scan:   {}", recursive);
    println!("Multi-Axis:       {}", multi_axis);
    println!("Overwrite:        {}", overwrite);

    let config = BatchJobConfig {
        model_path: model.map(|p| p.to_path_buf()),
        multi_axis,
        overwrite,
        ..Default::default()
    };

    let mut queue = BatchQueue::new();
    let count = queue.scan_directory(folder, recursive, &config);
    println!("Discovered {} videos requiring funscripts.", count);

    if count == 0 {
        println!("No videos to process. Exiting.");
        return Ok(());
    }

    let mut succeeded = 0;
    let mut failed = 0;

    for (idx, job) in queue.jobs.iter().enumerate() {
        println!("\n[{}/{}] Processing: {}", idx + 1, count, job.video_path.display());
        let width = if model.is_some() { 640 } else { 256 };
        let height = if model.is_some() { 640 } else { 256 };

        match run_generate(
            &job.video_path,
            Some(&job.output_path),
            model,
            config.conf,
            config.fps,
            width,
            height,
            2.0,
            3.0,
            2000,
            config.vr_mode,
            config.pov_mode,
            true,
            true,
            30.0,
            8.0,
            multi_axis,
            config.profile.short_name(),
        ) {
            Ok(_) => {
                succeeded += 1;
                println!("✓ Finished: {}", job.output_path.display());
            }
            Err(e) => {
                failed += 1;
                eprintln!("✗ Error processing {}: {e}", job.video_path.display());
            }
        }
    }

    println!("\n==================================================");
    println!("Batch Processing Summary");
    println!("==================================================");
    println!("Total Jobs:     {}", count);
    println!("Succeeded:      {}", succeeded);
    println!("Failed:         {}", failed);
    println!("==================================================");

    Ok(())
}

fn run_stash_list_missing(url: &str, api_key: Option<&str>, limit: u32) -> Result<()> {
    use stash::client::{StashClient, StashConfig};

    println!("==================================================");
    println!("Pulsar: Stash Media Server Integration");
    println!("==================================================");
    println!("Connecting to Stash endpoint: {}", url);

    let client = StashClient::new(StashConfig {
        endpoint: url.to_string(),
        api_key: api_key.map(|k| k.to_string()),
        auto_rescan: true,
    });

    match client.test_connection() {
        Ok(ver) => println!("Connected to Stash Server (version {})", ver),
        Err(e) => {
            eprintln!("Warning: Failed to query Stash version ({}). Proceeding with query...", e);
        }
    }

    println!("Querying for scenes missing funscripts (limit = {})...", limit);
    let scenes = client.find_scenes_missing_scripts(limit)?;

    println!("Found {} scenes missing interactive funscripts:", scenes.len());
    println!("{:<8} | {:<40} | Primary File Path", "ID", "Title");
    println!("{:-<8}-+-{:-<40}-+-{:-<30}", "", "", "");

    for scene in &scenes {
        let title = scene.title.as_deref().unwrap_or("<Untitled>");
        let file_path = scene.files.first().map(|f| f.path.as_str()).unwrap_or("<No file>");
        let trunc_title = if title.len() > 38 {
            format!("{}...", &title[..35])
        } else {
            title.to_string()
        };
        println!("{:<8} | {:<40} | {}", scene.id, trunc_title, file_path);
    }
    println!("==================================================");

    Ok(())
}

fn run_stash_auto_generate(
    url: &str,
    api_key: Option<&str>,
    limit: u32,
    model: Option<&Path>,
    multi_axis: bool,
) -> Result<()> {
    use stash::client::{StashClient, StashConfig};

    println!("==================================================");
    println!("Pulsar: Stash Media Server Auto-Generate");
    println!("==================================================");
    println!("Connecting to Stash endpoint: {}", url);

    let client = StashClient::new(StashConfig {
        endpoint: url.to_string(),
        api_key: api_key.map(|k| k.to_string()),
        auto_rescan: true,
    });

    let scenes = client.find_scenes_missing_scripts(limit)?;
    if scenes.is_empty() {
        println!("No scenes missing scripts found in Stash. Everything is synced!");
        return Ok(());
    }

    println!("Found {} scenes needing funscripts. Starting auto-generation...", scenes.len());
    let mut generated_paths = Vec::new();

    for (i, scene) in scenes.iter().enumerate() {
        let Some(file) = scene.files.first() else {
            continue;
        };
        let vid_path = Path::new(&file.path);
        if !vid_path.exists() {
            eprintln!("[{}/{}] File not accessible on local filesystem: {}", i + 1, scenes.len(), vid_path.display());
            continue;
        }

        let script_output = vid_path.with_extension("funscript");
        println!("\n[{}/{}] Generating script for: {}", i + 1, scenes.len(), vid_path.display());

        let width = if model.is_some() { 640 } else { 256 };
        let height = if model.is_some() { 640 } else { 256 };

        match run_generate(
            vid_path,
            Some(&script_output),
            model,
            0.35,
            30.0,
            width,
            height,
            2.0,
            3.0,
            2000,
            false,
            false,
            true,
            true,
            30.0,
            8.0,
            multi_axis,
            "default",
        ) {
            Ok(_) => {
                generated_paths.push(file.path.clone());
                println!("✓ Successfully generated {}", script_output.display());
            }
            Err(e) => {
                eprintln!("✗ Failed to generate {}: {e}", vid_path.display());
            }
        }
    }

    if !generated_paths.is_empty() {
        println!("\nTriggering Stash metadata rescan for {} scenes...", generated_paths.len());
        match client.trigger_metadata_scan(&generated_paths) {
            Ok(true) => println!("✓ Stash metadata rescan triggered successfully. Funscripts are now linked in Stash!"),
            Ok(false) => println!("Stash reported rescan task already in progress."),
            Err(e) => eprintln!("Notice: Could not trigger Stash rescan automatically: {e}"),
        }
    }

    println!("==================================================");
    println!("Stash Auto-Generation Complete ({} processed)", generated_paths.len());
    println!("==================================================");

    Ok(())
}

fn run_audio_synth(
    video_path: &Path,
    output_path: Option<&Path>,
    mode_str: &str,
    band_str: &str,
    axis_str: &str,
    threshold: f32,
    generate_bundle: bool,
) -> Result<()> {
    let t0 = Instant::now();
    println!("==================================================");
    println!("Pulsar: Audio Spectral Haptic Synthesizer");
    println!("Target: {}", video_path.display());
    println!("==================================================");

    println!("-> Extracting 16kHz PCM audio and computing multi-band DSP...");
    let waveform = crate::audio::extract_audio_waveform(video_path)?;

    if waveform.peaks.is_empty() {
        anyhow::bail!("No audio stream could be extracted from {}", video_path.display());
    }

    let duration_ms = (waveform.duration_secs * 1000.0).round() as i64;
    println!(
        "-> Audio extracted: {:.2}s ({} bins at 100 bins/sec)",
        waveform.duration_secs,
        waveform.peaks.len()
    );

    let effective_output = match output_path {
        Some(p) => p.to_path_buf(),
        None => {
            let mut out = video_path.to_path_buf();
            out.set_extension("funscript");
            out
        }
    };

    let empty_spec = crate::audio::SpectralAnalysis::default();
    let spec_ref = waveform.spectral.as_ref().unwrap_or(&empty_spec);

    if generate_bundle {
        println!("-> Synthesizing full 6-DOF multi-band companion bundle from spectral audio...");
        let bundle = crate::audio::generate_multiband_companion_bundle(
            spec_ref,
            &waveform.peaks,
            waveform.bins_per_sec,
            duration_ms,
        );

        println!("-> Saving multi-axis bundle to companion files...");
        bundle.save_bundle(&effective_output)?;

        let stroke_actions = bundle
            .channels
            .get(&AxisChannel::Stroke)
            .map(|s| s.actions.len())
            .unwrap_or(0);
        println!(
            "SUCCESS: Synthesized {} channels (Stroke: {} actions) in {:.2}s",
            bundle.channels.len(),
            stroke_actions,
            t0.elapsed().as_secs_f64()
        );
    } else {
        let band = match band_str.to_lowercase().as_str() {
            "sub" | "subbass" | "bass" => crate::audio::FrequencyBand::SubBass,
            "mid" | "mids" => crate::audio::FrequencyBand::Mid,
            "high" | "highs" => crate::audio::FrequencyBand::High,
            _ => crate::audio::FrequencyBand::FullSpectrum,
        };

        let mode = match mode_str.to_lowercase().as_str() {
            "env" | "envelope" => crate::audio::SpectralInfillMode::EnvelopeFollower,
            "vibe" | "osc" | "oscillation" => crate::audio::SpectralInfillMode::ModulatedOscillation,
            _ => crate::audio::SpectralInfillMode::RhythmicBeats,
        };

        let axis = match axis_str.to_lowercase().as_str() {
            "surge" => AxisChannel::Surge,
            "sway" => AxisChannel::Sway,
            "pitch" => AxisChannel::Pitch,
            "roll" => AxisChannel::Roll,
            "twist" => AxisChannel::Twist,
            "suction" => AxisChannel::Suction,
            _ => AxisChannel::Stroke,
        };

        let config = crate::audio::SpectralInfillConfig {
            band,
            target_axis: axis,
            mode,
            threshold,
            min_pos: 10,
            max_pos: 90,
            min_interval_ms: 180,
            modulation_freq_hz: 8.0,
        };

        println!(
            "-> Synthesizing {} channel using {:?} ({:?}, threshold {:.2})...",
            axis.display_name(),
            mode,
            band,
            threshold
        );

        let actions = crate::audio::synthesize_spectral_actions(
            spec_ref,
            &waveform.peaks,
            waveform.bins_per_sec,
            &config,
            0,
            duration_ms,
        );

        let mut script = Funscript::new(actions);
        script.sanitize();
        script.save(&effective_output)?;

        println!(
            "SUCCESS: Synthesized {} actions saved to {} in {:.2}s",
            script.actions.len(),
            effective_output.display(),
            t0.elapsed().as_secs_f64()
        );
    }

    Ok(())
}

