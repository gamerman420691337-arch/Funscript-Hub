//! Automatic Funscript detection and media library auditing engine.
//!
//! Scans directories for media files, inspects filesystem companion scripts,
//! categorizes coverage status (Missing, Partial, Complete), and formats reports.

use anyhow::Result;
use crate::batch::queue::{BatchJob, BatchJobConfig, BatchJobStatus};
use crate::funscript::AxisChannel;
use std::fs;
use std::path::{Path, PathBuf};

/// Script coverage status for a single media file
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScriptCoverage {
    /// Media file has no companion funscripts at all
    Missing,
    /// Media file has primary stroke or some channels, but is missing requested multi-axis companions
    Partial {
        existing: Vec<AxisChannel>,
        missing: Vec<AxisChannel>,
    },
    /// Media file is fully scripted for all requested axes
    Complete {
        channels: Vec<AxisChannel>,
    },
}

impl ScriptCoverage {
    pub fn is_missing(&self) -> bool {
        matches!(self, ScriptCoverage::Missing)
    }

    pub fn is_partial(&self) -> bool {
        matches!(self, ScriptCoverage::Partial { .. })
    }

    #[allow(dead_code)]
    pub fn is_complete(&self) -> bool {
        matches!(self, ScriptCoverage::Complete { .. })
    }

    #[allow(dead_code)]
    pub fn badge_str(&self) -> &'static str {
        match self {
            ScriptCoverage::Missing => "MISSING",
            ScriptCoverage::Partial { .. } => "PARTIAL",
            ScriptCoverage::Complete { .. } => "COVERED",
        }
    }
}

/// Audit entry for a single media file discovered on disk
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MediaAuditItem {
    pub video_path: PathBuf,
    pub file_size_bytes: u64,
    pub duration_secs: Option<f64>,
    pub coverage: ScriptCoverage,
    pub base_stem: String,
    pub primary_script_path: PathBuf,
}

impl MediaAuditItem {
    pub fn is_missing(&self) -> bool {
        self.coverage.is_missing()
    }

    pub fn is_partial(&self) -> bool {
        self.coverage.is_partial()
    }

    #[allow(dead_code)]
    pub fn is_complete(&self) -> bool {
        self.coverage.is_complete()
    }
}

/// Filter criteria for queueing jobs from an audit report
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EnqueueFilter {
    /// Only enqueue media files completely lacking funscripts
    MissingOnly,
    /// Enqueue media files missing funscripts or missing multi-axis companions
    MissingAndPartial,
    /// Enqueue all media files regardless of existing funscripts (overwrite mode)
    All,
}

/// Summary report of an audited media library directory
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LibraryAuditReport {
    pub scanned_root: PathBuf,
    pub total_media: usize,
    pub missing_count: usize,
    pub partial_count: usize,
    pub complete_count: usize,
    pub items: Vec<MediaAuditItem>,
}

impl LibraryAuditReport {
    /// Format and display a readable summary table to stdout
    pub fn print_summary(&self, missing_only: bool) {
        println!("================================================================================");
        println!("⚡ Pulsar: Media Library Funscript Audit Report");
        println!("================================================================================");
        println!("Scanned Root:     {}", self.scanned_root.display());
        println!("Total Media:      {}", self.total_media);
        println!("Missing Scripts:  {} (Unscripted media)", self.missing_count);
        println!("Partial Scripts:  {} (Missing companion multi-axis)", self.partial_count);
        println!("Complete Scripts: {}", self.complete_count);
        println!("================================================================================");

        if self.items.is_empty() {
            println!("No supported media files found in directory.");
            println!("================================================================================");
            return;
        }

        println!(
            "{:<9} | {:<10} | {:<42} | {:<12}",
            "STATUS", "SIZE", "FILE NAME", "CHANNELS"
        );
        println!("{:-<9}-+-{:-<10}-+-{:-<42}-+-{:-<12}", "", "", "", "");

        let items_to_display: Vec<&MediaAuditItem> = if missing_only {
            self.items.iter().filter(|i| i.is_missing()).collect()
        } else {
            self.items.iter().collect()
        };

        for item in &items_to_display {
            let status_badge = match &item.coverage {
                ScriptCoverage::Missing => "🔴 MISSING",
                ScriptCoverage::Partial { .. } => "🟡 PARTIAL",
                ScriptCoverage::Complete { .. } => "🟢 COVERED",
            };

            let size_str = format_bytes(item.file_size_bytes);
            let file_name = item
                .video_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let truncated_name = if file_name.len() > 40 {
                format!("{}...", &file_name[..37])
            } else {
                file_name.to_string()
            };

            let channel_detail = match &item.coverage {
                ScriptCoverage::Missing => "None".to_string(),
                ScriptCoverage::Complete { channels } => {
                    format!("{}/{} axes", channels.len(), channels.len())
                }
                ScriptCoverage::Partial { existing, missing } => {
                    format!("{}/{} (+{})", existing.len(), existing.len() + missing.len(), missing.len())
                }
            };

            println!(
                "{:<9} | {:>10} | {:<42} | {:<12}",
                status_badge, size_str, truncated_name, channel_detail
            );
        }

        println!("================================================================================");
        let coverage_pct = if self.total_media > 0 {
            (self.complete_count as f64 / self.total_media as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "Summary: {} of {} media files have funscripts ({:.1}% library coverage).",
            self.complete_count + self.partial_count,
            self.total_media,
            coverage_pct
        );
        if self.missing_count > 0 {
            println!(
                "💡 {} media files require generation. Use --generate to batch process them.",
                self.missing_count
            );
        }
        println!("================================================================================");
    }

    /// Serialize report into pretty JSON
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Format report as CSV
    pub fn to_csv(&self) -> String {
        let mut out = String::from("status,size_bytes,file_name,path,primary_script_path\n");
        for item in &self.items {
            let status = match &item.coverage {
                ScriptCoverage::Missing => "missing",
                ScriptCoverage::Partial { .. } => "partial",
                ScriptCoverage::Complete { .. } => "complete",
            };
            out.push_str(&format!(
                "\"{}\",{},\"{}\",\"{}\",\"{}\"\n",
                status,
                item.file_size_bytes,
                item.video_path.file_name().unwrap_or_default().to_string_lossy().replace('\"', "\"\""),
                item.video_path.display(),
                item.primary_script_path.display()
            ));
        }
        out
    }

    /// Convert audited items into batch execution queue jobs based on filter criteria
    pub fn into_batch_jobs(&self, filter: EnqueueFilter, _config: &BatchJobConfig) -> Vec<BatchJob> {
        let mut jobs = Vec::new();
        let mut job_id = 0;

        for item in &self.items {
            let should_enqueue = match filter {
                EnqueueFilter::MissingOnly => item.is_missing(),
                EnqueueFilter::MissingAndPartial => item.is_missing() || item.is_partial(),
                EnqueueFilter::All => true,
            };

            if should_enqueue {
                jobs.push(BatchJob {
                    id: job_id,
                    video_path: item.video_path.clone(),
                    output_path: item.primary_script_path.clone(),
                    status: BatchJobStatus::Queued,
                });
                job_id += 1;
            }
        }

        jobs
    }
}

/// Format raw byte count into human-readable representation
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Recursively or flatly scan a directory for media files and audit companion funscripts
pub fn scan_library(root_dir: &Path, recursive: bool, check_multi_axis: bool) -> LibraryAuditReport {
    let mut report = LibraryAuditReport {
        scanned_root: root_dir.to_path_buf(),
        ..Default::default()
    };

    let mut dirs_to_visit = vec![root_dir.to_path_buf()];
    let video_exts = ["mp4", "mkv", "webm", "avi", "mov", "m4v", "wmv"];

    while let Some(dir) = dirs_to_visit.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && recursive {
                dirs_to_visit.push(path);
            } else if path.is_file() {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                if video_exts.contains(&ext.as_str()) {
                    let file_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let base_stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();

                    let parent = path.parent().unwrap_or_else(|| Path::new("."));
                    let primary_script_path = path.with_extension("funscript");
                    let primary_exists = primary_script_path.exists();

                    let coverage = if check_multi_axis {
                        let mut existing = Vec::new();
                        let mut missing = Vec::new();

                        for axis in &AxisChannel::ALL {
                            let companion_file = if *axis == AxisChannel::Stroke {
                                format!("{}.funscript", base_stem)
                            } else {
                                format!("{}{}.funscript", base_stem, axis.file_suffix())
                            };
                            let candidate = parent.join(companion_file);
                            if candidate.exists() {
                                existing.push(*axis);
                            } else {
                                missing.push(*axis);
                            }
                        }

                        if existing.is_empty() {
                            ScriptCoverage::Missing
                        } else if missing.is_empty() {
                            ScriptCoverage::Complete { channels: existing }
                        } else {
                            ScriptCoverage::Partial { existing, missing }
                        }
                    } else if primary_exists {
                        ScriptCoverage::Complete {
                            channels: vec![AxisChannel::Stroke],
                        }
                    } else {
                        ScriptCoverage::Missing
                    };

                    match &coverage {
                        ScriptCoverage::Missing => report.missing_count += 1,
                        ScriptCoverage::Partial { .. } => report.partial_count += 1,
                        ScriptCoverage::Complete { .. } => report.complete_count += 1,
                    }

                    report.items.push(MediaAuditItem {
                        video_path: path,
                        file_size_bytes: file_size,
                        duration_secs: None,
                        coverage,
                        base_stem,
                        primary_script_path,
                    });
                    report.total_media += 1;
                }
            }
        }
    }

    // Sort items alphabetically by filename for clean reporting
    report.items.sort_by(|a, b| a.video_path.cmp(&b.video_path));

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "pulsar_det_{}_{}",
                name,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let _ = std::fs::create_dir_all(&p);
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_detect_missing_funscript() {
        let dir = TestDir::new("missing");
        let video_path = dir.path().join("scene1.mp4");
        File::create(&video_path).unwrap();

        let report = scan_library(dir.path(), false, false);
        assert_eq!(report.total_media, 1);
        assert_eq!(report.missing_count, 1);
        assert_eq!(report.complete_count, 0);
        assert!(report.items[0].is_missing());
        assert_eq!(report.items[0].coverage, ScriptCoverage::Missing);
    }

    #[test]
    fn test_detect_complete_funscript() {
        let dir = TestDir::new("complete");
        let video_path = dir.path().join("scene1.mp4");
        let script_path = dir.path().join("scene1.funscript");
        File::create(&video_path).unwrap();
        File::create(&script_path).unwrap();

        let report = scan_library(dir.path(), false, false);
        assert_eq!(report.total_media, 1);
        assert_eq!(report.missing_count, 0);
        assert_eq!(report.complete_count, 1);
        assert!(report.items[0].is_complete());
    }

    #[test]
    fn test_detect_partial_multiaxis() {
        let dir = TestDir::new("partial");
        let video_path = dir.path().join("scene1.mp4");
        let stroke_script = dir.path().join("scene1.funscript");
        let surge_script = dir.path().join("scene1.surge.funscript");
        File::create(&video_path).unwrap();
        File::create(&stroke_script).unwrap();
        File::create(&surge_script).unwrap();

        // When checking multi-axis, missing sway/pitch/roll/twist/suction makes it Partial
        let report = scan_library(dir.path(), false, true);
        assert_eq!(report.total_media, 1);
        assert_eq!(report.partial_count, 1);
        assert!(report.items[0].is_partial());
        if let ScriptCoverage::Partial { existing, missing } = &report.items[0].coverage {
            assert!(existing.contains(&AxisChannel::Stroke));
            assert!(existing.contains(&AxisChannel::Surge));
            assert!(missing.contains(&AxisChannel::Sway));
        } else {
            panic!("Expected ScriptCoverage::Partial");
        }
    }

    #[test]
    fn test_audit_report_filtering_and_json() {
        let dir = TestDir::new("filter_json");
        let v1 = dir.path().join("v1.mp4");
        let v2 = dir.path().join("v2.mp4");
        let s1 = dir.path().join("v1.funscript");
        File::create(&v1).unwrap();
        File::create(&s1).unwrap();
        File::create(&v2).unwrap();

        let report = scan_library(dir.path(), false, false);
        assert_eq!(report.total_media, 2);
        assert_eq!(report.complete_count, 1);
        assert_eq!(report.missing_count, 1);

        let json = report.to_json().unwrap();
        assert!(json.contains("v1.mp4"));
        assert!(json.contains("v2.mp4"));
        assert!(json.contains("Missing"));

        let csv = report.to_csv();
        assert!(csv.contains("v1.mp4"));
        assert!(csv.contains("v2.mp4"));
    }

    #[test]
    fn test_audit_report_to_batch_jobs() {
        let dir = TestDir::new("to_jobs");
        let v1 = dir.path().join("v1.mp4");
        let v2 = dir.path().join("v2.mp4");
        let s1 = dir.path().join("v1.funscript");
        File::create(&v1).unwrap();
        File::create(&s1).unwrap();
        File::create(&v2).unwrap();

        let report = scan_library(dir.path(), false, false);
        let cfg = BatchJobConfig::default();

        let missing_jobs = report.into_batch_jobs(EnqueueFilter::MissingOnly, &cfg);
        assert_eq!(missing_jobs.len(), 1);
        assert_eq!(missing_jobs[0].video_path, v2);

        let all_jobs = report.into_batch_jobs(EnqueueFilter::All, &cfg);
        assert_eq!(all_jobs.len(), 2);
    }
}
