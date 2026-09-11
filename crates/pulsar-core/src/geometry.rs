use crate::{
    ArtifactId, CoreError, EntityTrackId, EvidenceKind, FrameId, SourcePlacementId,
    SourceVersionId, TransformId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinateSpace {
    SourcePixels,
    ModelPixels,
    CropPixels,
    ProjectionNormalized,
    ViewportPixels,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CoordinateBoxWire", into = "CoordinateBoxWire")]
pub struct CoordinateBox {
    space: CoordinateSpace,
    x_min: f64,
    y_min: f64,
    x_max: f64,
    y_max: f64,
}

pub type BoundingBox = CoordinateBox;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoordinateBoxWire {
    space: CoordinateSpace,
    x_min: f64,
    y_min: f64,
    x_max: f64,
    y_max: f64,
}

impl CoordinateBox {
    pub fn new(
        space: CoordinateSpace,
        x_min: f64,
        y_min: f64,
        x_max: f64,
        y_max: f64,
    ) -> Result<Self, CoreError> {
        let values = [x_min, y_min, x_max, y_max];
        if !values.iter().all(|value| value.is_finite()) {
            return Err(CoreError::NonFinite);
        }
        if x_min >= x_max || y_min >= y_max {
            return Err(CoreError::InvalidGeometry(
                "box must have positive width and height".into(),
            ));
        }
        if space == CoordinateSpace::ProjectionNormalized
            && !values.iter().all(|v| (0.0..=1.0).contains(v))
        {
            return Err(CoreError::OutOfRange);
        }
        // Prevent finite endpoints from hiding an infinite span.
        if !(x_max - x_min).is_finite() || !(y_max - y_min).is_finite() {
            return Err(CoreError::NonFinite);
        }
        Ok(Self {
            space,
            x_min,
            y_min,
            x_max,
            y_max,
        })
    }
    pub const fn space(&self) -> CoordinateSpace {
        self.space
    }
    pub const fn x_min(&self) -> f64 {
        self.x_min
    }
    pub const fn y_min(&self) -> f64 {
        self.y_min
    }
    pub const fn x_max(&self) -> f64 {
        self.x_max
    }
    pub const fn y_max(&self) -> f64 {
        self.y_max
    }
    pub fn center(&self) -> (f64, f64) {
        (
            self.x_min + (self.x_max - self.x_min) / 2.0,
            self.y_min + (self.y_max - self.y_min) / 2.0,
        )
    }
    pub fn width(&self) -> f64 {
        self.x_max - self.x_min
    }
    pub fn height(&self) -> f64 {
        self.y_max - self.y_min
    }

    /// Returns no box if clipping removes the entire observation. Geometry and
    /// center are calculated from the same clipped endpoints.
    pub fn clipped_to(&self, width: f64, height: f64) -> Result<Option<Self>, CoreError> {
        if !width.is_finite() || !height.is_finite() {
            return Err(CoreError::NonFinite);
        }
        if width <= 0.0 || height <= 0.0 {
            return Err(CoreError::InvalidGeometry(
                "clip dimensions must be positive".into(),
            ));
        }
        if self.space == CoordinateSpace::ProjectionNormalized && (width != 1.0 || height != 1.0) {
            return Err(CoreError::InvalidGeometry(
                "normalized geometry clips to the unit rectangle".into(),
            ));
        }
        let x_min = self.x_min.max(0.0);
        let y_min = self.y_min.max(0.0);
        let x_max = self.x_max.min(width);
        let y_max = self.y_max.min(height);
        if x_min >= x_max || y_min >= y_max {
            return Ok(None);
        }
        Self::new(self.space, x_min, y_min, x_max, y_max).map(Some)
    }
}

impl TryFrom<CoordinateBoxWire> for CoordinateBox {
    type Error = CoreError;
    fn try_from(v: CoordinateBoxWire) -> Result<Self, Self::Error> {
        Self::new(v.space, v.x_min, v.y_min, v.x_max, v.y_max)
    }
}
impl From<CoordinateBox> for CoordinateBoxWire {
    fn from(v: CoordinateBox) -> Self {
        Self {
            space: v.space,
            x_min: v.x_min,
            y_min: v.y_min,
            x_max: v.x_max,
            y_max: v.y_max,
        }
    }
}

/// Every field participates in freshness. A numerically equal frame ordinal
/// from another placement, transform, or seek is not interchangeable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameContext {
    pub source_version: SourceVersionId,
    pub source_placement: SourcePlacementId,
    pub frame: FrameId,
    pub transform: TransformId,
    pub seek_generation: u64,
    pub request_generation: u64,
}

impl FrameContext {
    pub fn matches(&self, displayed: &Self) -> bool {
        self == displayed
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ConfidenceCalibration {
    Uncalibrated,
    Calibrated { artifact: ArtifactId },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ConfidenceWire", into = "ConfidenceWire")]
pub struct Confidence {
    value: f64,
    calibration: ConfidenceCalibration,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfidenceWire {
    value: f64,
    calibration: ConfidenceCalibration,
}

impl Confidence {
    pub fn new(value: f64, calibration: Option<ArtifactId>) -> Result<Self, CoreError> {
        if !value.is_finite() {
            return Err(CoreError::NonFinite);
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(CoreError::OutOfRange);
        }
        Ok(Self {
            value,
            calibration: calibration.map_or(ConfidenceCalibration::Uncalibrated, |artifact| {
                ConfidenceCalibration::Calibrated { artifact }
            }),
        })
    }
    pub const fn value(&self) -> f64 {
        self.value
    }
    pub fn calibration(&self) -> &ConfidenceCalibration {
        &self.calibration
    }
}
impl TryFrom<ConfidenceWire> for Confidence {
    type Error = CoreError;
    fn try_from(v: ConfidenceWire) -> Result<Self, Self::Error> {
        let artifact = match v.calibration {
            ConfidenceCalibration::Uncalibrated => None,
            ConfidenceCalibration::Calibrated { artifact } => Some(artifact),
        };
        Self::new(v.value, artifact)
    }
}
impl From<Confidence> for ConfidenceWire {
    fn from(v: Confidence) -> Self {
        Self {
            value: v.value,
            calibration: v.calibration,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    try_from = "DetectionObservationWire",
    into = "DetectionObservationWire"
)]
pub struct DetectionObservation {
    context: FrameContext,
    bounds: CoordinateBox,
    track: Option<EntityTrackId>,
    evidence: EvidenceKind,
    confidence: Confidence,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DetectionObservationWire {
    context: FrameContext,
    bounds: CoordinateBox,
    track: Option<EntityTrackId>,
    evidence: EvidenceKind,
    confidence: Confidence,
}

impl DetectionObservation {
    pub fn new(
        context: FrameContext,
        bounds: CoordinateBox,
        track: Option<EntityTrackId>,
        evidence: EvidenceKind,
        confidence: Confidence,
    ) -> Result<Self, CoreError> {
        if evidence == EvidenceKind::Unavailable {
            return Err(CoreError::InvalidGeometry(
                "unavailable evidence cannot carry a detection box".into(),
            ));
        }
        Ok(Self {
            context,
            bounds,
            track,
            evidence,
            confidence,
        })
    }
    pub fn context(&self) -> &FrameContext {
        &self.context
    }
    pub fn bounds(&self) -> &CoordinateBox {
        &self.bounds
    }
    pub fn track(&self) -> Option<&EntityTrackId> {
        self.track.as_ref()
    }
    pub const fn evidence(&self) -> EvidenceKind {
        self.evidence
    }
    pub fn confidence(&self) -> &Confidence {
        &self.confidence
    }
    pub fn matches_frame(&self, displayed: &FrameContext) -> bool {
        self.context.matches(displayed)
    }
}
impl TryFrom<DetectionObservationWire> for DetectionObservation {
    type Error = CoreError;
    fn try_from(v: DetectionObservationWire) -> Result<Self, Self::Error> {
        Self::new(v.context, v.bounds, v.track, v.evidence, v.confidence)
    }
}
impl From<DetectionObservation> for DetectionObservationWire {
    fn from(v: DetectionObservation) -> Self {
        Self {
            context: v.context,
            bounds: v.bounds,
            track: v.track,
            evidence: v.evidence,
            confidence: v.confidence,
        }
    }
}
