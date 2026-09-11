//! Checked detector decoding, bounded identity tracking, and sparse optical flow.
//!
//! All geometry is source-pixel geometry. Optical flow estimates image motion,
//! never physical pose. See ADR 0002 for assumptions and qualification limits.

use crate::{
    Confidence, CoordinateBox, CoordinateSpace, CoreError, DetectionObservation, EntityTrackId,
    EvidenceKind, FrameContext,
};
use std::collections::BTreeMap;

pub const MAX_DETECTIONS: usize = 256;
pub const MAX_FLOW_POINTS: usize = 64;
const MAX_TENSOR_ELEMENTS: usize = 10_000_000;
const MAX_IMAGE_PIXELS: usize = 1_048_576;

fn invalid(message: &str) -> CoreError {
    CoreError::InvalidGeometry(message.into())
}
fn fraction(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectorLayout {
    /// [1, 4 + classes, anchors], pixel center-x/center-y/width/height.
    YoloChannelMajor,
    /// [1, detections, 6], pixel x-min/y-min/x-max/y-max/score/class.
    YoloEndToEnd,
}

/// Exact resize/letterbox transform used when producing a model input.
/// Crops and arbitrary projections need their own declared transforms.
#[derive(Clone, Debug)]
pub struct DetectorTransform {
    source_width: u32,
    source_height: u32,
    model_width: u32,
    model_height: u32,
    scale: [f64; 2],
    padding: [f64; 2],
}
impl DetectorTransform {
    pub fn new(
        source_width: u32,
        source_height: u32,
        model_width: u32,
        model_height: u32,
        scale: [f64; 2],
        padding: [f64; 2],
    ) -> Result<Self, CoreError> {
        if [source_width, source_height, model_width, model_height].contains(&0)
            || [source_width, source_height, model_width, model_height]
                .iter()
                .any(|x| *x > 65_536)
            || scale.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || padding.iter().any(|v| !v.is_finite() || *v < 0.0)
            || f64::from(source_width) * scale[0] + padding[0] > f64::from(model_width) + 1e-6
            || f64::from(source_height) * scale[1] + padding[1] > f64::from(model_height) + 1e-6
        {
            return Err(invalid(
                "invalid declared detector resize/letterbox transform",
            ));
        }
        Ok(Self {
            source_width,
            source_height,
            model_width,
            model_height,
            scale,
            padding,
        })
    }
    pub fn resize(
        source_width: u32,
        source_height: u32,
        model_width: u32,
        model_height: u32,
    ) -> Result<Self, CoreError> {
        Self::new(
            source_width,
            source_height,
            model_width,
            model_height,
            [
                f64::from(model_width) / f64::from(source_width),
                f64::from(model_height) / f64::from(source_height),
            ],
            [0.0, 0.0],
        )
    }
    fn source_box(&self, values: [f64; 4]) -> Result<Option<CoordinateBox>, CoreError> {
        let bounds = CoordinateBox::new(
            CoordinateSpace::SourcePixels,
            (values[0] - self.padding[0]) / self.scale[0],
            (values[1] - self.padding[1]) / self.scale[1],
            (values[2] - self.padding[0]) / self.scale[0],
            (values[3] - self.padding[1]) / self.scale[1],
        )?;
        bounds.clipped_to(f64::from(self.source_width), f64::from(self.source_height))
    }
    pub fn source_dimensions(&self) -> (u32, u32) {
        (self.source_width, self.source_height)
    }
    pub fn model_dimensions(&self) -> (u32, u32) {
        (self.model_width, self.model_height)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalDetection {
    bounds: CoordinateBox,
    class_id: u32,
    score: f64,
}
impl CanonicalDetection {
    pub fn new(bounds: CoordinateBox, class_id: u32, score: f64) -> Result<Self, CoreError> {
        if bounds.space() != CoordinateSpace::SourcePixels || !fraction(score) || class_id >= 256 {
            return Err(invalid(
                "detections require source-pixel geometry, declared class, and bounded score",
            ));
        }
        Ok(Self {
            bounds,
            class_id,
            score,
        })
    }
    pub fn bounds(&self) -> &CoordinateBox {
        &self.bounds
    }
    pub fn class_id(&self) -> u32 {
        self.class_id
    }
    /// Uncalibrated detector score, not a probability of correctness.
    pub fn score(&self) -> f64 {
        self.score
    }
}

/// Decode only the declared schema, validate every tensor element, invert the
/// declared image transform, clip, then perform class-aware deterministic NMS.
/// No shape guessing, implicit normalized coordinates, or partial invalid rows.
pub fn decode_detections(
    values: &[f32],
    shape: &[usize],
    layout: DetectorLayout,
    classes: usize,
    transform: &DetectorTransform,
    threshold: f64,
    nms_iou: f64,
) -> Result<Vec<CanonicalDetection>, CoreError> {
    if classes == 0
        || classes > 256
        || shape.len() != 3
        || shape[0] != 1
        || !fraction(threshold)
        || !fraction(nms_iou)
    {
        return Err(invalid("invalid detection schema or threshold"));
    }
    let rows = match layout {
        DetectorLayout::YoloChannelMajor if shape[1] == classes + 4 => shape[2],
        DetectorLayout::YoloEndToEnd if shape[2] == 6 => shape[1],
        _ => return Err(invalid("detection tensor does not match declared decoder")),
    };
    let elements = shape
        .iter()
        .try_fold(1usize, |a, b| a.checked_mul(*b))
        .ok_or(CoreError::Overflow)?;
    if rows > 100_000
        || elements > MAX_TENSOR_ELEMENTS
        || elements != values.len()
        || values.iter().any(|v| !v.is_finite())
    {
        return Err(invalid(
            "invalid detector tensor length, finiteness, or resource budget",
        ));
    }
    let mut candidates = Vec::new();
    for row in 0..rows {
        let (bbox, score, class) = match layout {
            DetectorLayout::YoloChannelMajor => {
                let coords = [
                    values[row] as f64,
                    values[rows + row] as f64,
                    values[2 * rows + row] as f64,
                    values[3 * rows + row] as f64,
                ];
                if coords[2] <= 0.0 || coords[3] <= 0.0 {
                    return Err(invalid("detector box extent must be positive"));
                }
                let mut class = 0;
                let mut score = -1.0f64;
                for index in 0..classes {
                    let candidate = values[(4 + index) * rows + row] as f64;
                    if !fraction(candidate) {
                        return Err(invalid("invalid detector class score"));
                    }
                    if candidate > score {
                        class = index as u32;
                        score = candidate;
                    }
                }
                (
                    [
                        coords[0] - coords[2] / 2.0,
                        coords[1] - coords[3] / 2.0,
                        coords[0] + coords[2] / 2.0,
                        coords[1] + coords[3] / 2.0,
                    ],
                    score,
                    class,
                )
            }
            DetectorLayout::YoloEndToEnd => {
                let raw = &values[row * 6..row * 6 + 6];
                let class = raw[5] as f64;
                if class < 0.0
                    || class >= classes as f64
                    || class.fract() != 0.0
                    || !fraction(raw[4] as f64)
                {
                    return Err(invalid("invalid end-to-end class or score"));
                }
                (
                    [raw[0] as f64, raw[1] as f64, raw[2] as f64, raw[3] as f64],
                    raw[4] as f64,
                    class as u32,
                )
            }
        };
        // Validate coordinates even for low-score rows. Malformed model output
        // is a failed attempt, not a silently skipped observation.
        let bounds = transform.source_box(bbox)?;
        if score >= threshold {
            if let Some(bounds) = bounds {
                candidates.push(CanonicalDetection::new(bounds, class, score)?);
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.class_id.cmp(&b.class_id))
            .then_with(|| a.bounds.x_min().total_cmp(&b.bounds.x_min()))
            .then_with(|| a.bounds.y_min().total_cmp(&b.bounds.y_min()))
            .then_with(|| a.bounds.x_max().total_cmp(&b.bounds.x_max()))
            .then_with(|| a.bounds.y_max().total_cmp(&b.bounds.y_max()))
    });
    let mut selected: Vec<CanonicalDetection> = Vec::new();
    for candidate in candidates {
        if selected.iter().any(|old| {
            old.class_id == candidate.class_id && iou(&old.bounds, &candidate.bounds) > nms_iou
        }) {
            continue;
        }
        selected.push(candidate);
        if selected.len() == MAX_DETECTIONS {
            break;
        }
    }
    Ok(selected)
}
fn iou(a: &CoordinateBox, b: &CoordinateBox) -> f64 {
    let intersection = (a.x_max().min(b.x_max()) - a.x_min().max(b.x_min())).max(0.0)
        * (a.y_max().min(b.y_max()) - a.y_min().max(b.y_min())).max(0.0);
    let union = a.width() * a.height() + b.width() * b.height() - intersection;
    if union > 0.0 {
        intersection / union
    } else {
        0.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TrackerConfig {
    /// Elapsed source frame ordinals on explicitly absent updates, not call count.
    /// Frames intentionally omitted between positive observations are unknown,
    /// not known absences.
    pub max_missed_frames: u32,
    pub max_tracks: usize,
    /// Fraction of source-image diagonal, per source frame.
    pub max_center_distance: f64,
    /// Near-equal association costs are not resolved by input order.
    pub ambiguity_margin: f64,
}
impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            max_missed_frames: 3,
            max_tracks: MAX_DETECTIONS,
            max_center_distance: 0.25,
            ambiguity_margin: 0.02,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackingReview {
    SceneCut,
    SourceContextChanged,
    MissingDetection,
    AmbiguousAssociation,
    TrackBudgetReached,
}
#[derive(Clone, Debug)]
pub struct TrackingUpdate {
    pub observations: Vec<DetectionObservation>,
    pub reviews: Vec<TrackingReview>,
    classes: BTreeMap<EntityTrackId, u32>,
}
impl TrackingUpdate {
    pub fn class_for(&self, track: &EntityTrackId) -> Option<u32> {
        self.classes.get(track).copied()
    }
}
#[derive(Clone, Debug)]
struct Track {
    id: EntityTrackId,
    class_id: u32,
    bounds: CoordinateBox,
    velocity: [f64; 2],
    score: f64,
    missed: u64,
    last_observed_frame: u64,
    last_observed_center: [f64; 2],
}
#[derive(Clone, Debug)]
pub struct TargetTracker {
    config: TrackerConfig,
    tracks: Vec<Track>,
    last: Option<FrameContext>,
    dimensions: Option<(u32, u32)>,
    next_id: u64,
    namespace: EntityTrackId,
}
impl TargetTracker {
    /// Creates an isolated local tracker. Engine persistence should instead use
    /// an attempt-unique namespace via with_namespace.
    pub fn new(config: TrackerConfig) -> Result<Self, CoreError> {
        Self::with_namespace(EntityTrackId::new("local")?, config)
    }
    pub fn with_namespace(
        namespace: EntityTrackId,
        config: TrackerConfig,
    ) -> Result<Self, CoreError> {
        if namespace.as_str().len() > 96 {
            return Err(invalid("track namespace exceeds identity budget"));
        }
        if config.max_tracks == 0
            || config.max_tracks > MAX_DETECTIONS
            || config.max_missed_frames > 10_000
            || !fraction(config.max_center_distance)
            || config.max_center_distance == 0.0
            || !fraction(config.ambiguity_margin)
        {
            return Err(invalid(
                "invalid tracker budget or association configuration",
            ));
        }
        Ok(Self {
            config,
            tracks: Vec::new(),
            last: None,
            dimensions: None,
            next_id: 0,
            namespace,
        })
    }
    pub fn update(
        &mut self,
        context: FrameContext,
        width: u32,
        height: u32,
        detections: &[CanonicalDetection],
        scene_cut: bool,
    ) -> Result<TrackingUpdate, CoreError> {
        // Validate and stage the transition so malformed requests cannot age
        // tracks, replace identities, or partially update the last frame.
        if width == 0
            || height == 0
            || width > 65_536
            || height > 65_536
            || detections.len() > MAX_DETECTIONS
        {
            return Err(invalid("invalid tracker geometry or detection budget"));
        }
        for detection in detections {
            if detection.bounds.x_min() < 0.0
                || detection.bounds.y_min() < 0.0
                || detection.bounds.x_max() > f64::from(width)
                || detection.bounds.y_max() > f64::from(height)
            {
                return Err(invalid(
                    "tracking observations must already be clipped to source",
                ));
            }
        }
        let changed = self.last.as_ref().is_some_and(|old| {
            old.source_version != context.source_version
                || old.source_placement != context.source_placement
                || old.transform != context.transform
                || old.seek_generation != context.seek_generation
        }) || self.dimensions.is_some_and(|size| size != (width, height));
        if !changed
            && !scene_cut
            && self
                .last
                .as_ref()
                .is_some_and(|old| context.frame <= old.frame)
        {
            return Err(invalid(
                "tracking frames must increase within a seek/transform generation",
            ));
        }
        let mut staged = self.clone();
        let result = staged.advance(context, width, height, detections, scene_cut, changed)?;
        *self = staged;
        Ok(result)
    }
    fn advance(
        &mut self,
        context: FrameContext,
        width: u32,
        height: u32,
        detections: &[CanonicalDetection],
        scene_cut: bool,
        changed: bool,
    ) -> Result<TrackingUpdate, CoreError> {
        let mut reviews = Vec::new();
        if scene_cut {
            self.tracks.clear();
            reviews.push(TrackingReview::SceneCut);
        }
        if changed {
            self.tracks.clear();
            reviews.push(TrackingReview::SourceContextChanged);
        }
        let delta = if scene_cut || changed {
            1
        } else {
            self.last
                .as_ref()
                .map_or(1, |old| context.frame.value() - old.frame.value())
        };
        let frame = context.frame.value();
        let diagonal = f64::from(width).hypot(f64::from(height));
        // Skipped source frames are not observed absences. Positive spatial
        // associations may bridge intentional sampling; only explicit missing
        // updates age tracks below. Already expired tracks have been removed.
        let predicted: Vec<_> = self
            .tracks
            .iter()
            .map(|track| {
                translated(
                    &track.bounds,
                    track.velocity[0] * delta as f64,
                    track.velocity[1] * delta as f64,
                    width,
                    height,
                )
            })
            .collect::<Result<_, _>>()?;
        let mut pairs = Vec::<(f64, usize, usize)>::new();
        for (t, track) in self.tracks.iter().enumerate() {
            let Some(bounds) = &predicted[t] else {
                continue;
            };
            let center = bounds.center();
            for (d, detection) in detections.iter().enumerate() {
                if track.class_id != detection.class_id {
                    continue;
                }
                let other = detection.bounds.center();
                let distance = (center.0 - other.0).hypot(center.1 - other.1) / diagonal;
                if distance <= self.config.max_center_distance * (delta as f64).min(4.0) {
                    pairs.push((distance, t, d));
                }
            }
        }
        pairs.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        let mut ambiguous_tracks = vec![false; self.tracks.len()];
        let mut ambiguous_detections = vec![false; detections.len()];
        for t in 0..self.tracks.len() {
            let close: Vec<_> = pairs.iter().filter(|pair| pair.1 == t).take(2).collect();
            if close.len() == 2 && close[1].0 - close[0].0 <= self.config.ambiguity_margin {
                ambiguous_tracks[t] = true;
                ambiguous_detections[close[0].2] = true;
                ambiguous_detections[close[1].2] = true;
            }
        }
        for d in 0..detections.len() {
            let close: Vec<_> = pairs.iter().filter(|pair| pair.2 == d).take(2).collect();
            if close.len() == 2 && close[1].0 - close[0].0 <= self.config.ambiguity_margin {
                ambiguous_detections[d] = true;
                ambiguous_tracks[close[0].1] = true;
                ambiguous_tracks[close[1].1] = true;
            }
        }
        if ambiguous_tracks.iter().any(|v| *v) || ambiguous_detections.iter().any(|v| *v) {
            reviews.push(TrackingReview::AmbiguousAssociation);
        }
        let mut matched_tracks = vec![false; self.tracks.len()];
        let mut matched_detections = vec![false; detections.len()];
        for (_, t, d) in pairs {
            if matched_tracks[t]
                || matched_detections[d]
                || ambiguous_tracks[t]
                || ambiguous_detections[d]
            {
                continue;
            }
            let track = &mut self.tracks[t];
            let detection = &detections[d];
            let center = detection.bounds.center();
            let elapsed = frame
                .checked_sub(track.last_observed_frame)
                .ok_or(CoreError::Overflow)?
                .max(1) as f64;
            track.velocity = [
                (center.0 - track.last_observed_center[0]) / elapsed,
                (center.1 - track.last_observed_center[1]) / elapsed,
            ];
            track.bounds = detection.bounds.clone();
            track.score = detection.score;
            track.missed = 0;
            track.last_observed_frame = frame;
            track.last_observed_center = [center.0, center.1];
            matched_tracks[t] = true;
            matched_detections[d] = true;
        }
        for (t, track) in self.tracks.iter_mut().enumerate() {
            if matched_tracks[t] {
                continue;
            }
            track.missed = track.missed.checked_add(delta).ok_or(CoreError::Overflow)?;
            if let Some(bounds) = &predicted[t] {
                track.bounds = bounds.clone();
            } else {
                track.missed = u64::from(self.config.max_missed_frames) + 1;
            }
        }
        if self.tracks.iter().any(|track| track.missed > 0)
            || (detections.is_empty() && self.last.is_some())
        {
            reviews.push(TrackingReview::MissingDetection);
        }
        self.tracks
            .retain(|track| track.missed <= u64::from(self.config.max_missed_frames));
        for (index, detection) in detections.iter().enumerate() {
            if matched_detections[index] {
                continue;
            }
            if self.tracks.len() == self.config.max_tracks {
                // Fresh observations win capacity over predictions, but an
                // admitted observed identity is never silently overwritten.
                if let Some(oldest) = self
                    .tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, track)| track.missed > 0)
                    .max_by_key(|(_, track)| track.missed)
                    .map(|(i, _)| i)
                {
                    self.tracks.remove(oldest);
                } else {
                    if !reviews.contains(&TrackingReview::TrackBudgetReached) {
                        reviews.push(TrackingReview::TrackBudgetReached);
                    }
                    continue;
                }
            }
            let id = EntityTrackId::new(format!("{}.{}", self.namespace, self.next_id))?;
            self.next_id = self.next_id.checked_add(1).ok_or(CoreError::Overflow)?;
            let center = detection.bounds.center();
            self.tracks.push(Track {
                id,
                class_id: detection.class_id,
                bounds: detection.bounds.clone(),
                velocity: [0.0, 0.0],
                score: detection.score,
                missed: 0,
                last_observed_frame: frame,
                last_observed_center: [center.0, center.1],
            });
        }
        let mut observations = Vec::with_capacity(self.tracks.len());
        let mut classes = BTreeMap::new();
        for track in &self.tracks {
            let predicted = track.missed > 0;
            // Decay is a heuristic score; it is never marked calibrated.
            let confidence = if predicted {
                track.score / (1.0 + track.missed as f64)
            } else {
                track.score
            };
            observations.push(DetectionObservation::new(
                context.clone(),
                track.bounds.clone(),
                Some(track.id.clone()),
                if predicted {
                    EvidenceKind::Predicted
                } else {
                    EvidenceKind::Observed
                },
                Confidence::new(confidence, None)?,
            )?);
            classes.insert(track.id.clone(), track.class_id);
        }
        self.last = Some(context);
        self.dimensions = Some((width, height));
        Ok(TrackingUpdate {
            observations,
            reviews,
            classes,
        })
    }
}
fn translated(
    bounds: &CoordinateBox,
    dx: f64,
    dy: f64,
    width: u32,
    height: u32,
) -> Result<Option<CoordinateBox>, CoreError> {
    CoordinateBox::new(
        CoordinateSpace::SourcePixels,
        bounds.x_min() + dx,
        bounds.y_min() + dy,
        bounds.x_max() + dx,
        bounds.y_max() + dy,
    )?
    .clipped_to(f64::from(width), f64::from(height))
}

#[derive(Clone, Copy, Debug)]
pub struct FlowConfig {
    pub patch_radius: u32,
    pub search_radius: u32,
    pub max_iterations: u32,
    /// Minimum eigenvalue of average template gradient covariance.
    pub min_texture: f64,
    pub max_mean_absolute_error: f64,
    pub max_forward_backward_error: f64,
}
impl Default for FlowConfig {
    fn default() -> Self {
        Self {
            patch_radius: 3,
            search_radius: 6,
            max_iterations: 12,
            min_texture: 16.0,
            max_mean_absolute_error: 20.0,
            max_forward_backward_error: 1.0,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowRejection {
    OutOfBounds,
    LowTexture,
    AmbiguousPattern,
    PhotometricMismatch,
    NonConvergent,
    ForwardBackwardMismatch,
}
#[derive(Clone, Debug)]
pub struct PointFlow {
    pub source: [f64; 2],
    /// Source pixels; None for rejected evidence, never silently zero-filled.
    pub displacement: Option<[f64; 2]>,
    pub forward_backward_error: Option<f64>,
    pub mean_absolute_error: Option<f64>,
    pub rejection: Option<FlowRejection>,
}

/// Bounded translational Lucas-Kanade, integer SSD initialization, bilinear
/// refinement, independent current-to-previous solve, and cycle rejection.
/// No neural model, learned visibility, or 3-D interpretation is implied.
pub fn track_points_fb(
    previous: &[u8],
    current: &[u8],
    width: u32,
    height: u32,
    points: &[[f64; 2]],
    config: FlowConfig,
) -> Result<Vec<PointFlow>, CoreError> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(CoreError::Overflow)?;
    if width == 0
        || height == 0
        || pixels > MAX_IMAGE_PIXELS
        || previous.len() != pixels
        || current.len() != pixels
        || points.len() > MAX_FLOW_POINTS
        || config.patch_radius == 0
        || config.patch_radius > 5
        || config.search_radius == 0
        || config.search_radius > 8
        || config.max_iterations == 0
        || config.max_iterations > 20
        || !config.min_texture.is_finite()
        || config.min_texture <= 0.0
        || !config.max_mean_absolute_error.is_finite()
        || config.max_mean_absolute_error <= 0.0
        || config.max_mean_absolute_error > 255.0
        || !config.max_forward_backward_error.is_finite()
        || config.max_forward_backward_error <= 0.0
        || config.max_forward_backward_error > 8.0
        || points.iter().any(|p| p.iter().any(|v| !v.is_finite()))
    {
        return Err(invalid(
            "invalid optical-flow buffers, points, configuration, or work budget",
        ));
    }
    let mut result = Vec::with_capacity(points.len());
    for &point in points {
        let mut flow = PointFlow {
            source: point,
            displacement: None,
            forward_backward_error: None,
            mean_absolute_error: None,
            rejection: None,
        };
        match solve_translation(
            previous,
            current,
            width as usize,
            height as usize,
            point,
            config,
        ) {
            Err(reason) => flow.rejection = Some(reason),
            Ok((forward, error)) => {
                flow.mean_absolute_error = Some(error);
                let destination = [point[0] + forward[0], point[1] + forward[1]];
                // Reverse inputs and solve at the FORWARD DESTINATION. Reusing
                // the forward field is not a forward-backward check.
                match solve_translation(
                    current,
                    previous,
                    width as usize,
                    height as usize,
                    destination,
                    config,
                ) {
                    Err(reason) => flow.rejection = Some(reason),
                    Ok((backward, _)) => {
                        let cycle = (forward[0] + backward[0]).hypot(forward[1] + backward[1]);
                        flow.forward_backward_error = Some(cycle);
                        if cycle > config.max_forward_backward_error {
                            flow.rejection = Some(FlowRejection::ForwardBackwardMismatch);
                        } else {
                            flow.displacement = Some(forward);
                        }
                    }
                }
            }
        }
        result.push(flow);
    }
    Ok(result)
}
fn sample(image: &[u8], width: usize, x: f64, y: f64) -> f64 {
    let ix = x.floor() as usize;
    let iy = y.floor() as usize;
    let fx = x - ix as f64;
    let fy = y - iy as f64;
    let a = image[iy * width + ix] as f64;
    let b = image[iy * width + ix + 1] as f64;
    let c = image[(iy + 1) * width + ix] as f64;
    let d = image[(iy + 1) * width + ix + 1] as f64;
    (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
}
fn solve_translation(
    previous: &[u8],
    current: &[u8],
    width: usize,
    height: usize,
    point: [f64; 2],
    config: FlowConfig,
) -> Result<([f64; 2], f64), FlowRejection> {
    let radius = config.patch_radius as i32;
    let search = config.search_radius as i32;
    let margin = f64::from(radius + search + 2);
    if point[0] < margin
        || point[1] < margin
        || point[0] + margin >= width as f64
        || point[1] + margin >= height as f64
    {
        return Err(FlowRejection::OutOfBounds);
    }
    let mut template = Vec::with_capacity(((radius * 2 + 1) * (radius * 2 + 1)) as usize);
    let (mut xx, mut xy, mut yy) = (0.0, 0.0, 0.0);
    for oy in -radius..=radius {
        for ox in -radius..=radius {
            let x = point[0] + f64::from(ox);
            let y = point[1] + f64::from(oy);
            let gx =
                (sample(previous, width, x + 1.0, y) - sample(previous, width, x - 1.0, y)) / 2.0;
            let gy =
                (sample(previous, width, x, y + 1.0) - sample(previous, width, x, y - 1.0)) / 2.0;
            let value = sample(previous, width, x, y);
            template.push((x, y, value, gx, gy));
            xx += gx * gx;
            xy += gx * gy;
            yy += gy * gy;
        }
    }
    let count = template.len() as f64;
    let min_eigen = ((xx + yy) - ((xx - yy) * (xx - yy) + 4.0 * xy * xy).sqrt()) / (2.0 * count);
    let determinant = xx * yy - xy * xy;
    if min_eigen < config.min_texture || determinant <= f64::EPSILON {
        return Err(FlowRejection::LowTexture);
    }
    let (mut best, mut second) = (f64::INFINITY, f64::INFINITY);
    let mut displacement = [0.0, 0.0];
    for dy in -search..=search {
        for dx in -search..=search {
            let mut cost = 0.0;
            for &(x, y, value, _, _) in &template {
                let difference =
                    value - sample(current, width, x + f64::from(dx), y + f64::from(dy));
                cost += difference * difference;
            }
            if cost < best {
                second = best;
                best = cost;
                displacement = [f64::from(dx), f64::from(dy)];
            } else if cost < second {
                second = cost;
            }
        }
    }
    if second - best <= 1e-9 {
        return Err(FlowRejection::AmbiguousPattern);
    }
    let mut converged = false;
    for _ in 0..config.max_iterations {
        let (mut bx, mut by) = (0.0, 0.0);
        for &(x, y, value, gx, gy) in &template {
            let residual = value - sample(current, width, x + displacement[0], y + displacement[1]);
            bx += gx * residual;
            by += gy * residual;
        }
        let step = [
            (yy * bx - xy * by) / determinant,
            (xx * by - xy * bx) / determinant,
        ];
        if !step.iter().all(|v| v.is_finite()) {
            return Err(FlowRejection::NonConvergent);
        }
        displacement[0] += step[0];
        displacement[1] += step[1];
        if displacement
            .iter()
            .any(|v| v.abs() > f64::from(search) + 0.5)
        {
            return Err(FlowRejection::NonConvergent);
        }
        if step[0].hypot(step[1]) < 0.01 {
            converged = true;
            break;
        }
    }
    if !converged {
        return Err(FlowRejection::NonConvergent);
    }
    let error = template
        .iter()
        .map(|&(x, y, value, _, _)| {
            (value - sample(current, width, x + displacement[0], y + displacement[1])).abs()
        })
        .sum::<f64>()
        / count;
    if error > config.max_mean_absolute_error {
        return Err(FlowRejection::PhotometricMismatch);
    }
    Ok((displacement, error))
}

#[derive(Clone, Debug, PartialEq)]
pub struct MotionSeparation {
    pub target_image_translation: Option<[f64; 2]>,
    /// A background image-motion proxy, not a measured physical camera pose.
    pub background_image_translation: Option<[f64; 2]>,
    pub target_relative_to_background: Option<[f64; 2]>,
    pub target_accepted_points: usize,
    pub background_accepted_points: usize,
}
/// Median translation of at least three accepted points per region. Region
/// selection is the caller's explicit responsibility. Subtraction is withheld
/// without background evidence or if either region is internally inconsistent.
pub fn separate_target_motion(
    target: &[PointFlow],
    background: &[PointFlow],
) -> Result<MotionSeparation, CoreError> {
    if target.len() > MAX_FLOW_POINTS || background.len() > MAX_FLOW_POINTS {
        return Err(invalid("motion separation exceeds bounded point budget"));
    }
    fn summarize(points: &[PointFlow]) -> Result<(Option<[f64; 2]>, usize), CoreError> {
        let mut accepted = Vec::new();
        for point in points {
            if let Some(delta) = point.displacement {
                if !delta.iter().all(|v| v.is_finite() && v.abs() <= 8.5)
                    || point.rejection.is_some()
                    || point.source.iter().any(|v| !v.is_finite() || *v < 0.0)
                    || !point
                        .forward_backward_error
                        .is_some_and(|v| v.is_finite() && v >= 0.0)
                    || !point
                        .mean_absolute_error
                        .is_some_and(|v| v.is_finite() && (0.0..=255.0).contains(&v))
                {
                    return Err(invalid("inconsistent accepted point-flow evidence"));
                }
                accepted.push(delta);
            }
        }
        let count = accepted.len();
        if count < 3 {
            return Ok((None, count));
        }
        let mut xs: Vec<_> = accepted.iter().map(|d| d[0]).collect();
        let mut ys: Vec<_> = accepted.iter().map(|d| d[1]).collect();
        xs.sort_by(f64::total_cmp);
        ys.sort_by(f64::total_cmp);
        let median = |values: &[f64]| {
            if values.len() % 2 == 0 {
                (values[values.len() / 2 - 1] + values[values.len() / 2]) / 2.0
            } else {
                values[values.len() / 2]
            }
        };
        let center = [median(&xs), median(&ys)];
        // A translation model must explain at least two thirds of support
        // within one source pixel. Otherwise camera/target separation is unknown.
        let inliers = accepted
            .iter()
            .filter(|d| (d[0] - center[0]).hypot(d[1] - center[1]) <= 1.0)
            .count();
        Ok((
            if inliers * 3 >= count * 2 {
                Some(center)
            } else {
                None
            },
            count,
        ))
    }
    let (target_image_translation, target_accepted_points) = summarize(target)?;
    let (background_image_translation, background_accepted_points) = summarize(background)?;
    let target_relative_to_background = target_image_translation
        .zip(background_image_translation)
        .map(|(a, b)| [a[0] - b[0], a[1] - b[1]]);
    Ok(MotionSeparation {
        target_image_translation,
        background_image_translation,
        target_relative_to_background,
        target_accepted_points,
        background_accepted_points,
    })
}

/// Seed a bounded grid strictly inside a checked source-pixel region. No clamped
/// synthetic rectangle is manufactured from a reversed or empty request.
pub fn seed_points_in_box(
    bounds: &CoordinateBox,
    width: u32,
    height: u32,
    count: usize,
) -> Result<Vec<[f64; 2]>, CoreError> {
    if count > MAX_FLOW_POINTS
        || width == 0
        || height == 0
        || bounds.space() != CoordinateSpace::SourcePixels
        || bounds.x_min() < 0.0
        || bounds.y_min() < 0.0
        || bounds.x_max() > f64::from(width)
        || bounds.y_max() > f64::from(height)
    {
        return Err(invalid("invalid source ROI or point budget"));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let columns = (count as f64).sqrt().ceil() as usize;
    let rows = count.div_ceil(columns);
    Ok((0..count)
        .map(|index| {
            [
                bounds.x_min() + bounds.width() * ((index % columns) as f64 + 0.5) / columns as f64,
                bounds.y_min() + bounds.height() * ((index / columns) as f64 + 0.5) / rows as f64,
            ]
        })
        .collect())
}

/// Seed at most count points from an exact grayscale mask; zero requests and
/// empty foreground are valid empty results. Dimensions are checked even then.
pub fn seed_points_in_mask(
    mask: &[u8],
    width: u32,
    height: u32,
    count: usize,
) -> Result<Vec<[f64; 2]>, CoreError> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(CoreError::Overflow)?;
    if width == 0
        || height == 0
        || pixels > MAX_IMAGE_PIXELS
        || mask.len() != pixels
        || count > MAX_FLOW_POINTS
    {
        return Err(invalid("invalid mask shape or point budget"));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let available = mask.iter().filter(|value| **value > 127).count();
    let take = count.min(available);
    if take == 0 {
        return Ok(Vec::new());
    }
    let mut points = Vec::with_capacity(take);
    let mut foreground_index = 0usize;
    for (index, &value) in mask.iter().enumerate() {
        if value <= 127 {
            continue;
        }
        if points.len() < take && foreground_index == points.len() * available / take {
            points.push([
                (index % width as usize) as f64,
                (index / width as usize) as f64,
            ]);
        }
        foreground_index += 1;
    }
    Ok(points)
}
