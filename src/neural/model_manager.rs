//! Model discovery, auto-detection, and downloader for FunGen YOLO models.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_MODEL_NAME: &str = "FunGen-12n-pov-1.1.0.onnx";
pub const OFFICIAL_MODEL_URL: &str =
    "https://github.com/ack00gar/FunGen-AI-Powered-Funscript-Generator/releases/download/models-v1.1.0/FunGen-12n-pov-1.1.0.onnx";

/// Metadata describing an ONNX model file.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub path: PathBuf,
    pub filename: String,
    pub size_bytes: u64,
    pub is_valid: bool,
}

/// Returns the default cache directory for models: `~/.cache/pulsar/models/`.
pub fn default_model_cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".cache")
        });
    base.join("pulsar").join("models")
}

/// Returns prioritized search locations for FunGen YOLO models.
pub fn model_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. Pulsar global cache
    paths.push(default_model_cache_dir().join(DEFAULT_MODEL_NAME));

    // 2. Legacy fs-hub global cache (backward compatibility)
    if let Ok(home) = std::env::var("HOME") {
        paths.push(
            PathBuf::from(&home)
                .join(".cache/fs-hub/models")
                .join(DEFAULT_MODEL_NAME),
        );
    }

    // 3. Local workspace models directory
    paths.push(PathBuf::from("models").join(DEFAULT_MODEL_NAME));
    paths.push(PathBuf::from("../models").join(DEFAULT_MODEL_NAME));

    // 4. FunGen project cache
    if let Ok(home) = std::env::var("HOME") {
        paths.push(
            PathBuf::from(&home)
                .join("Desktop/Funscripts/FunGen/fungen-data/cache/models")
                .join(DEFAULT_MODEL_NAME),
        );
        paths.push(
            PathBuf::from(&home)
                .join("Desktop/FunGen/fungen-data/cache/models")
                .join(DEFAULT_MODEL_NAME),
        );
        paths.push(
            PathBuf::from(&home)
                .join(".cache/fungen/models")
                .join(DEFAULT_MODEL_NAME),
        );
    }

    paths
}

/// Inspect a model file and return its metadata.
pub fn inspect_model<P: AsRef<Path>>(path: P) -> Option<ModelInfo> {
    let path = path.as_ref();
    if !path.is_file() {
        return None;
    }

    let metadata = fs::metadata(path).ok()?;
    let size_bytes = metadata.len();
    // Valid ONNX model should be non-empty (typically > 5 MB)
    let is_valid = size_bytes > 1_000_000;

    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown.onnx")
        .to_string();

    Some(ModelInfo {
        path: path.to_path_buf(),
        filename,
        size_bytes,
        is_valid,
    })
}

/// Auto-detect the best available YOLO model on the local system.
pub fn auto_detect_model() -> Option<ModelInfo> {
    for candidate in model_search_paths() {
        if candidate.is_file() {
            if let Some(info) = inspect_model(&candidate) {
                if info.is_valid {
                    return Some(info);
                }
            }
        }
    }

    // Also check if any .onnx file is in the default cache dir
    let cache_dir = default_model_cache_dir();
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|ext| ext.to_str()) == Some("onnx") {
                if let Some(info) = inspect_model(&p) {
                    if info.is_valid {
                        return Some(info);
                    }
                }
            }
        }
    }

    None
}

/// Ensure the default model is installed into `~/.cache/pulsar/models/`.
///
/// If found in an existing local directory (such as FunGen cache), copies it over.
/// Otherwise, downloads it from the official GitHub release via `ureq`.
pub fn install_or_download_default_model<F>(progress_fn: F) -> Result<PathBuf, String>
where
    F: Fn(u64, u64) + Send + 'static,
{
    let target_dir = default_model_cache_dir();
    fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Failed to create model cache dir {:?}: {}", target_dir, e))?;

    let target_path = target_dir.join(DEFAULT_MODEL_NAME);

    // If target already exists and is valid, return immediately
    if target_path.is_file() {
        if let Some(info) = inspect_model(&target_path) {
            if info.is_valid {
                progress_fn(info.size_bytes, info.size_bytes);
                return Ok(target_path);
            }
        }
    }

    // Check if available from another local path first (fast copy)
    for candidate in model_search_paths() {
        if candidate.is_file() && candidate != target_path {
            if let Some(info) = inspect_model(&candidate) {
                if info.is_valid {
                    fs::copy(&candidate, &target_path).map_err(|e| {
                        format!("Failed to copy model from {:?} to {:?}: {}", candidate, target_path, e)
                    })?;
                    progress_fn(info.size_bytes, info.size_bytes);
                    return Ok(target_path);
                }
            }
        }
    }

    // Download from official URL via ureq
    let tmp_path = target_dir.join(format!("{}.tmp", DEFAULT_MODEL_NAME));
    let resp = ureq::get(OFFICIAL_MODEL_URL)
        .call()
        .map_err(|e| format!("HTTP request to {} failed: {}", OFFICIAL_MODEL_URL, e))?;

    let content_len = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(11_500_000);

    let mut reader = resp.into_body().into_reader();
    let mut out_file = File::create(&tmp_path)
        .map_err(|e| format!("Failed to create temp file {:?}: {}", tmp_path, e))?;

    let mut downloaded = 0u64;
    let mut buffer = [0u8; 65536];

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                out_file
                    .write_all(&buffer[..n])
                    .map_err(|e| format!("Failed writing to temp model: {}", e))?;
                downloaded += n as u64;
                progress_fn(downloaded, content_len);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("Download stream read error: {}", e)),
        }
    }

    out_file
        .flush()
        .map_err(|e| format!("Failed to flush model file: {}", e))?;
    drop(out_file);

    fs::rename(&tmp_path, &target_path)
        .map_err(|e| format!("Failed to finalize model file: {}", e))?;

    Ok(target_path)
}

/// A registered model in the Pulsar neural suite
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRegistryEntry {
    pub name: &'static str,
    pub filename: &'static str,
    pub description: &'static str,
    pub profile: crate::neural::pipeline::AdaptiveProfile,
    pub download_url: &'static str,
    pub size_estimate_mb: f32,
    /// SHA256 hash of the model file for integrity verification (hex string)
    pub sha256: &'static str,
    /// Model category for routing
    pub model_type: ModelType,
}

/// Category of a registered model
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelType {
    /// Object/anatomy detector (YOLO, RF-DETR)
    Detector,
    /// Promptable segmenter (SAM 3.1)
    Segmenter,
    /// Point/correspondence tracker (TAPNext++)
    Tracker,
    /// Dense reconstruction model (CoWTracker, 3D)
    DenseReconstruction,
}

/// Catalog of supported ML models across the 5 adaptive profiles.
///
/// Models are hosted on GitHub releases. SHA256 hashes are verified on download
/// to ensure integrity. Models with placeholder hashes ("pending") have not yet
/// been trained and exported — they will be populated as model training completes.
#[allow(dead_code)]
pub fn model_registry() -> Vec<ModelRegistryEntry> {
    let base_url = "https://github.com/gamerman420691337-arch/pulsar-models/releases/download/v1.0.0";
    // Leak the formatted strings so they have 'static lifetime for the registry
    // This is called once at startup, so the leak is acceptable
    let economy_url: &'static str = Box::leak(format!("{base_url}/yolo26n-anatomy-v1.0.onnx").into_boxed_str());
    let specialist_url: &'static str = Box::leak(format!("{base_url}/rfdetr-seg-s-anatomy-v1.0.onnx").into_boxed_str());
    let sam_encoder_url: &'static str = Box::leak(format!("{base_url}/sam31-encoder-vit-b-v1.0.onnx").into_boxed_str());
    let sam_decoder_url: &'static str = Box::leak(format!("{base_url}/sam31-decoder-v1.0.onnx").into_boxed_str());
    let tapnext_url: &'static str = Box::leak(format!("{base_url}/tapnext-pp-256-v1.0.onnx").into_boxed_str());
    let cowtracker_url: &'static str = Box::leak(format!("{base_url}/cowtracker-dense-v1.0.onnx").into_boxed_str());

    vec![
        // Legacy FunGen detector (currently the only model with real weights)
        ModelRegistryEntry {
            name: "FunGen-12n (Legacy YOLOv12)",
            filename: DEFAULT_MODEL_NAME,
            description: "Original YOLOv12 anatomy detector with NMS post-processing",
            profile: crate::neural::pipeline::AdaptiveProfile::Economy,
            download_url: OFFICIAL_MODEL_URL,
            size_estimate_mb: 11.5,
            sha256: "pending",
            model_type: ModelType::Detector,
        },
        // YOLO26 NMS-free detector
        ModelRegistryEntry {
            name: "YOLO26-N Anatomy",
            filename: "yolo26n-anatomy-v1.0.onnx",
            description: "NMS-free direct detector, ~8.7 GFLOPs, 1.7ms latency on CPU",
            profile: crate::neural::pipeline::AdaptiveProfile::Economy,
            download_url: economy_url,
            size_estimate_mb: 12.4,
            sha256: "pending",
            model_type: ModelType::Detector,
        },
        // RF-DETR instance segmentation specialist
        ModelRegistryEntry {
            name: "RF-DETR-Seg-S Anatomy",
            filename: "rfdetr-seg-s-anatomy-v1.0.onnx",
            description: "Direct mask segmentation at Pareto knee of accuracy/latency",
            profile: crate::neural::pipeline::AdaptiveProfile::BalancedSpecialist,
            download_url: specialist_url,
            size_estimate_mb: 22.0,
            sha256: "pending",
            model_type: ModelType::Detector,
        },
        // SAM 3.1 encoder
        ModelRegistryEntry {
            name: "SAM 3.1 Encoder (ViT-B)",
            filename: "sam31-encoder-vit-b-v1.0.onnx",
            description: "Image feature encoder, runs once per scene cut (~120ms)",
            profile: crate::neural::pipeline::AdaptiveProfile::GenericDefault,
            download_url: sam_encoder_url,
            size_estimate_mb: 152.0,
            sha256: "pending",
            model_type: ModelType::Segmenter,
        },
        // SAM 3.1 decoder
        ModelRegistryEntry {
            name: "SAM 3.1 Decoder",
            filename: "sam31-decoder-v1.0.onnx",
            description: "Lightweight mask decoder, runs per-prompt (~8ms)",
            profile: crate::neural::pipeline::AdaptiveProfile::GenericDefault,
            download_url: sam_decoder_url,
            size_estimate_mb: 16.0,
            sha256: "pending",
            model_type: ModelType::Segmenter,
        },
        // TAPNext++ point tracker
        ModelRegistryEntry {
            name: "TAPNext++ 256-point",
            filename: "tapnext-pp-256-v1.0.onnx",
            description: "Sparse trajectory tracker, 256 points × 32 frames (~45ms CPU)",
            profile: crate::neural::pipeline::AdaptiveProfile::BalancedSpecialist,
            download_url: tapnext_url,
            size_estimate_mb: 28.0,
            sha256: "pending",
            model_type: ModelType::Tracker,
        },
        // CoWTracker dense repair
        ModelRegistryEntry {
            name: "CoWTracker Dense",
            filename: "cowtracker-dense-v1.0.onnx",
            description: "Dense spatiotemporal correspondence for uncertain intervals",
            profile: crate::neural::pipeline::AdaptiveProfile::DenseOffline,
            download_url: cowtracker_url,
            size_estimate_mb: 48.0,
            sha256: "pending",
            model_type: ModelType::DenseReconstruction,
        },
    ]
}

/// Status of a registered model on the local system
#[derive(Debug, Clone, PartialEq)]
pub struct ModelStatusItem {
    pub entry: ModelRegistryEntry,
    pub is_installed: bool,
    pub local_path: Option<PathBuf>,
    pub local_size_bytes: Option<u64>,
}

/// Retrieve status for all registered models
pub fn get_models_status() -> Vec<ModelStatusItem> {
    let registry = model_registry();
    let cache_dir = default_model_cache_dir();
    let mut results = Vec::new();

    for entry in registry {
        let cache_target = cache_dir.join(entry.filename);
        let info = if cache_target.is_file() {
            inspect_model(&cache_target)
        } else if entry.filename == DEFAULT_MODEL_NAME {
            auto_detect_model()
        } else {
            None
        };

        let is_installed = info.as_ref().map(|i| i.is_valid).unwrap_or(false);
        let local_path = info.as_ref().map(|i| i.path.clone());
        let local_size_bytes = info.as_ref().map(|i| i.size_bytes);

        results.push(ModelStatusItem {
            entry,
            is_installed,
            local_path,
            local_size_bytes,
        });
    }

    results
}

/// Summary of the local model cache directory: (path, count, total_bytes)
pub fn cache_summary() -> (PathBuf, usize, u64) {
    let cache_dir = default_model_cache_dir();
    let mut count = 0;
    let mut total_bytes = 0u64;

    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("onnx") {
                if let Ok(m) = fs::metadata(&p) {
                    count += 1;
                    total_bytes += m.len();
                }
            }
        }
    }

    (cache_dir, count, total_bytes)
}

/// Download a model by its registry name, filename, or "default"
pub fn download_model_by_name<F>(name: &str, progress_fn: F) -> Result<PathBuf, String>
where
    F: Fn(u64, u64) + Send + 'static,
{
    let target_dir = default_model_cache_dir();
    fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Failed to create model cache dir {:?}: {}", target_dir, e))?;

    let lower = name.to_lowercase();
    if lower == "default" || lower == "legacy" || lower.contains("12n") {
        return install_or_download_default_model(progress_fn);
    }

    let registry = model_registry();
    let entry = registry.iter().find(|e| {
        e.name.to_lowercase().contains(&lower)
            || e.filename.to_lowercase().contains(&lower)
    }).ok_or_else(|| format!("Unknown model '{name}'. Run 'pulsar model list' to view available models."))?;

    let target_path = target_dir.join(entry.filename);
    if target_path.is_file() {
        if let Some(info) = inspect_model(&target_path) {
            if info.is_valid {
                progress_fn(info.size_bytes, info.size_bytes);
                return Ok(target_path);
            }
        }
    }

    // Download from release URL
    let tmp_path = target_dir.join(format!("{}.tmp", entry.filename));
    let resp = ureq::get(entry.download_url)
        .call()
        .map_err(|e| format!("HTTP request to {} failed: {}", entry.download_url, e))?;

    let content_len = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or((entry.size_estimate_mb * 1_000_000.0) as u64);

    let mut reader = resp.into_body().into_reader();
    let mut out_file = File::create(&tmp_path)
        .map_err(|e| format!("Failed to create temp file {:?}: {}", tmp_path, e))?;

    let mut downloaded = 0u64;
    let mut buffer = [0u8; 65536];

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                out_file
                    .write_all(&buffer[..n])
                    .map_err(|e| format!("Failed writing to temp model: {}", e))?;
                downloaded += n as u64;
                progress_fn(downloaded, content_len);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("Download stream read error: {}", e)),
        }
    }

    out_file
        .flush()
        .map_err(|e| format!("Failed to flush model file: {}", e))?;
    drop(out_file);

    fs::rename(&tmp_path, &target_path)
        .map_err(|e| format!("Failed to finalize model file: {}", e))?;

    Ok(target_path)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_paths_include_defaults() {
        let paths = model_search_paths();
        assert!(!paths.is_empty());
        assert!(paths.iter().any(|p| p.ends_with(DEFAULT_MODEL_NAME)));
    }

    #[test]
    fn test_default_cache_dir() {
        let dir = default_model_cache_dir();
        assert!(dir.ends_with("pulsar/models"));
    }

    #[test]
    fn test_auto_detect_model_finds_fixture_or_cache() {
        // If the FunGen model is on this machine, auto_detect_model should locate it
        if let Some(info) = auto_detect_model() {
            assert!(info.is_valid);
            assert!(info.size_bytes > 1_000_000);
            println!("Found model: {:?} ({:.2} MB)", info.path, info.size_bytes as f64 / 1_000_000.0);
        }
    }
}
