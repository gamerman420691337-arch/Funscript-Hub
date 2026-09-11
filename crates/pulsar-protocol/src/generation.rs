//! Additive modality, review, and diagnostics contracts. Input declarations do
//! not authorize source reads, approve candidates, or qualify inference quality.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_PROMPT_BYTES: usize = 4096;
pub const MAX_REVIEW_FLAGS: usize = 1024;
pub const MAX_DIAGNOSTIC_ISSUES: usize = 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "modality",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum GenerationInput {
    Text {
        prompt: String,
    },
    Image {
        source_version: SourceVersionId,
        prompt: String,
    },
    Patterns {
        patterns: Vec<PatternSpec>,
    },
    Audio {
        source_version: SourceVersionId,
        mapping: AudioMapping,
        mode: AudioMode,
    },
}

impl std::fmt::Debug for GenerationInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Text { .. } => "Text",
            Self::Image { .. } => "Image",
            Self::Patterns { .. } => "Patterns",
            Self::Audio { .. } => "Audio",
        })
    }
}

impl GenerationInput {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Text { prompt } | Self::Image { prompt, .. } => {
                bounded_text(prompt, MAX_PROMPT_BYTES, "generation prompt")?;
            }
            Self::Patterns { patterns } => {
                if patterns.is_empty() || patterns.len() > Axis::ALL.len() {
                    return Err(ProtocolError::invalid(
                        "pattern input requires between one and six axes",
                    ));
                }
                let mut axes = BTreeSet::new();
                if patterns.iter().any(|pattern| !axes.insert(pattern.axis())) {
                    return Err(ProtocolError::invalid(
                        "pattern input contains duplicate axes",
                    ));
                }
            }
            Self::Audio { .. } => {}
        }
        Ok(())
    }

    /// Text and explicit patterns use an engine-materialized immutable source
    /// artifact. Media modalities must bind to the supplied source snapshot.
    pub fn source_version(&self) -> Option<&SourceVersionId> {
        match self {
            Self::Image { source_version, .. } | Self::Audio { source_version, .. } => {
                Some(source_version)
            }
            Self::Text { .. } | Self::Patterns { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    Envelope,
    Beats,
}

/// Stable machine-readable code, not a free-form user prompt or private trace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReviewReason(String);

impl ReviewReason {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
            })
        {
            return Err(ProtocolError::invalid(
                "review reason must be a bounded lowercase machine-readable code",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for ReviewReason {
    type Error = ProtocolError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<ReviewReason> for String {
    fn from(value: ReviewReason) -> Self {
        value.0
    }
}
impl std::fmt::Display for ReviewReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "ReviewFlagWire", into = "ReviewFlagWire")]
pub struct ReviewFlag {
    pub axis: Option<Axis>,
    pub range: TimeRange,
    pub reason: ReviewReason,
    pub evidence: EvidenceKind,
    pub confidence: Option<Confidence>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewFlagWire {
    axis: Option<Axis>,
    range: TimeRange,
    reason: ReviewReason,
    evidence: EvidenceKind,
    confidence: Option<Confidence>,
}

impl ReviewFlag {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.range.start() < ProjectTime::ZERO || self.range.end() <= self.range.start() {
            return Err(ProtocolError::invalid(
                "review span must be nonnegative and nonempty",
            ));
        }
        if self.confidence.as_ref().is_some_and(|confidence| {
            !matches!(
                confidence.calibration(),
                ConfidenceCalibration::Uncalibrated
            )
        }) {
            return Err(ProtocolError::invalid(
                "review confidence is uncalibrated; this contract cannot claim calibration",
            ));
        }
        Ok(())
    }
}
impl TryFrom<ReviewFlagWire> for ReviewFlag {
    type Error = ProtocolError;
    fn try_from(value: ReviewFlagWire) -> Result<Self, Self::Error> {
        let flag = Self {
            axis: value.axis,
            range: value.range,
            reason: value.reason,
            evidence: value.evidence,
            confidence: value.confidence,
        };
        flag.validate()?;
        Ok(flag)
    }
}
impl From<ReviewFlag> for ReviewFlagWire {
    fn from(value: ReviewFlag) -> Self {
        Self {
            axis: value.axis,
            range: value.range,
            reason: value.reason,
            evidence: value.evidence,
            confidence: value.confidence,
        }
    }
}

pub fn validate_reviews(review: &[ReviewFlag]) -> Result<(), ProtocolError> {
    if review.len() > MAX_REVIEW_FLAGS {
        return Err(ProtocolError::invalid("too many candidate review spans"));
    }
    for flag in review {
        flag.validate()?;
    }
    Ok(())
}

pub fn validate_merge_axes(axes: &[Axis]) -> Result<(), ProtocolError> {
    if axes.is_empty() || axes.len() > Axis::ALL.len() {
        return Err(ProtocolError::invalid(
            "candidate merge requires between one and six axes",
        ));
    }
    let mut unique = BTreeSet::new();
    if axes.iter().any(|axis| !unique.insert(*axis)) {
        return Err(ProtocolError::invalid(
            "candidate merge contains duplicate axes",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticIssue {
    pub code: String,
    pub message: String,
    pub axis: Option<Axis>,
    pub range: Option<TimeRange>,
}

impl DiagnosticIssue {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        bounded_text(&self.code, 64, "diagnostic code")?;
        bounded_text(&self.message, 512, "diagnostic message")?;
        if self
            .range
            .as_ref()
            .is_some_and(|range| range.start() < ProjectTime::ZERO || range.end() <= range.start())
        {
            return Err(ProtocolError::invalid(
                "diagnostic span must be nonnegative and nonempty",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsReport {
    pub project_id: ProjectId,
    pub revision: RevisionId,
    pub candidate_id: Option<CandidateId>,
    pub base_revision: Option<RevisionId>,
    pub protected_regions: Vec<ProtectedRegion>,
    pub issues: Vec<DiagnosticIssue>,
}

impl DiagnosticsReport {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.candidate_id.is_some() != self.base_revision.is_some() {
            return Err(ProtocolError::invalid(
                "candidate diagnostics require candidate and base revision together",
            ));
        }
        if self.protected_regions.len() > 1024 || self.issues.len() > MAX_DIAGNOSTIC_ISSUES {
            return Err(ProtocolError::invalid(
                "diagnostics exceed the bounded control path",
            ));
        }
        for issue in &self.issues {
            issue.validate()?;
        }
        // Reserve room for the outer response identity and tagged envelope. The
        // engine must additionally bound the fully serialized response.
        let encoded = serde_json::to_vec(self)
            .map_err(|_| ProtocolError::invalid("diagnostics cannot be serialized"))?;
        if encoded.len() > MAX_CONTROL_BYTES.saturating_sub(4096) {
            return Err(ProtocolError::invalid(
                "diagnostics exceed the bounded control frame",
            ));
        }
        Ok(())
    }
}
