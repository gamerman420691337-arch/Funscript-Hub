//! Frame/evidence freshness and bounded immutable artifact consumption.

use anyhow::{bail, Result};
use pulsar_protocol::*;
use sha2::{Digest, Sha256};
use std::io::Read;

pub const MAX_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Default)]
pub struct PreviewState {
    expected: Option<(FrameContext, SourceTimestamp)>,
    displayed: Option<PreviewResult>,
}

impl PreviewState {
    pub fn begin(&mut self, expected: FrameContext, requested_source_time: SourceTimestamp) {
        self.expected = Some((expected, requested_source_time));
        self.displayed = None;
    }
    pub fn clear(&mut self) {
        self.expected = None;
        self.displayed = None;
    }
    pub fn accept(&mut self, result: PreviewResult) -> bool {
        if !self
            .expected
            .as_ref()
            .is_some_and(|(expected, source_time)| {
                result.matches_seek_request(expected, source_time)
            })
        {
            return false;
        }
        self.displayed = Some(result);
        true
    }
    pub fn displayed(&self) -> Option<&PreviewResult> {
        self.displayed.as_ref()
    }
}

pub(crate) struct DecodedFrame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Artifact paths come only from an authenticated engine response. Files are
/// not decoded as arbitrary image formats; their checked raw layout is the ABI.
pub(crate) fn decode_frame(frame: &FrameArtifact) -> Result<DecodedFrame> {
    frame.validate(MAX_PREVIEW_BYTES)?;
    let metadata = std::fs::symlink_metadata(&frame.artifact.path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != frame.artifact.identity.byte_len
    {
        bail!("Preview artifact changed or is not an immutable regular file");
    }
    let mut bytes = Vec::with_capacity(frame.artifact.identity.byte_len as usize);
    std::fs::File::open(&frame.artifact.path)?
        .take(MAX_PREVIEW_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != frame.artifact.identity.byte_len
        || format!("{:x}", Sha256::digest(&bytes)) != frame.artifact.identity.sha256
    {
        bail!("Preview artifact identity mismatch");
    }
    let width = usize::try_from(frame.width)?;
    let height = usize::try_from(frame.height)?;
    let rgba_len = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .filter(|n| *n as u64 <= MAX_PREVIEW_BYTES)
        .ok_or_else(|| anyhow::anyhow!("Preview RGBA budget exceeded"))?;
    let stride = usize::try_from(frame.stride_bytes)?;
    let channels = frame.format.channels() as usize;
    let mut rgba = Vec::with_capacity(rgba_len);
    for row in bytes.chunks_exact(stride).take(height) {
        for pixel in row[..width * channels].chunks_exact(channels) {
            match frame.format {
                PixelFormat::Rgb8 => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
                PixelFormat::Rgba8 => rgba.extend_from_slice(pixel),
                PixelFormat::Gray8 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
            }
        }
    }
    Ok(DecodedFrame {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> FrameContext {
        FrameContext {
            source_version: SourceVersionId::new("source").unwrap(),
            source_placement: SourcePlacementId::new("placement").unwrap(),
            frame: FrameId::new(4),
            transform: TransformId::new("source-plane").unwrap(),
            seek_generation: 3,
            request_generation: 8,
        }
    }

    fn empty_preview(context: FrameContext) -> PreviewResult {
        PreviewResult {
            context,
            source_time: SourceTimestamp::new(1, 2).unwrap(),
            requested_source_time: SourceTimestamp::new(1, 2).unwrap(),
            source_frame_index: 4,
            observations: vec![],
            frame: None,
            analysis: AnalysisStatus::Available,
        }
    }

    #[test]
    fn seek_source_transform_and_request_changes_reject_late_results() {
        let original = context();
        let mut altered = vec![];
        let mut seek = original.clone();
        seek.seek_generation += 1;
        altered.push(seek);
        let mut request = original.clone();
        request.request_generation += 1;
        altered.push(request);
        let mut transform = original.clone();
        transform.transform = TransformId::new("crop").unwrap();
        altered.push(transform);
        let mut source = original.clone();
        source.source_version = SourceVersionId::new("other-source").unwrap();
        altered.push(source);
        for current in altered {
            let mut state = PreviewState::default();
            state.begin(current, SourceTimestamp::new(1, 2).unwrap());
            assert!(!state.accept(empty_preview(original.clone())));
            assert!(state.displayed().is_none());
        }
    }

    #[test]
    fn absent_detections_clear_previous_observations() {
        let mut state = PreviewState::default();
        state.begin(context(), SourceTimestamp::new(1, 2).unwrap());
        let mut observed = empty_preview(context());
        observed.observations.push(
            DetectionObservation::new(
                context(),
                CoordinateBox::new(CoordinateSpace::ProjectionNormalized, 0.1, 0.2, 0.3, 0.4)
                    .unwrap(),
                None,
                EvidenceKind::Observed,
                Confidence::new(0.8, None).unwrap(),
            )
            .unwrap(),
        );
        assert!(state.accept(observed));
        assert_eq!(state.displayed().unwrap().observations.len(), 1);
        assert!(state.accept(empty_preview(context())));
        assert!(state.displayed().unwrap().observations.is_empty());
        let mut next = context();
        next.request_generation += 1;
        state.begin(next, SourceTimestamp::new(1, 2).unwrap());
        assert!(state.displayed().is_none());
    }

    #[test]
    fn moving_boxes_and_actual_decoder_frame_replace_previous_evidence() {
        let mut state = PreviewState::default();
        let mut request = context();
        request.frame = FrameId::new(0);
        state.begin(request, SourceTimestamp::new(1, 2).unwrap());
        let mut result = empty_preview(context());
        result.observations.push(
            DetectionObservation::new(
                context(),
                CoordinateBox::new(CoordinateSpace::ProjectionNormalized, 0.6, 0.2, 0.8, 0.4)
                    .unwrap(),
                None,
                EvidenceKind::Observed,
                Confidence::new(0.8, None).unwrap(),
            )
            .unwrap(),
        );
        assert!(state.accept(result));
        assert_eq!(state.displayed().unwrap().context.frame.value(), 4);
        assert_eq!(
            state.displayed().unwrap().observations[0].bounds().x_min(),
            0.6
        );
        let mut mismatched = empty_preview(context());
        let mut wrong_frame = context();
        wrong_frame.frame = FrameId::new(999);
        mismatched.observations.push(
            DetectionObservation::new(
                wrong_frame,
                CoordinateBox::new(CoordinateSpace::ProjectionNormalized, 0.1, 0.2, 0.3, 0.4)
                    .unwrap(),
                None,
                EvidenceKind::Observed,
                Confidence::new(0.8, None).unwrap(),
            )
            .unwrap(),
        );
        assert!(!state.accept(mismatched));
    }

    #[test]
    fn raw_artifact_hash_is_checked_before_display() {
        let path =
            std::env::temp_dir().join(format!("pulsar-preview-test-{}", uuid::Uuid::new_v4()));
        let bytes = [1u8, 2, 3];
        std::fs::write(&path, bytes).unwrap();
        let frame = FrameArtifact {
            artifact: ArtifactReference {
                path: path.clone(),
                identity: ArtifactIdentity {
                    artifact_id: ArtifactId::new("frame").unwrap(),
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                    byte_len: 3,
                },
            },
            width: 1,
            height: 1,
            stride_bytes: 3,
            format: PixelFormat::Rgb8,
        };
        assert_eq!(decode_frame(&frame).unwrap().rgba, vec![1, 2, 3, 255]);
        std::fs::write(&path, [3u8, 2, 1]).unwrap();
        assert!(decode_frame(&frame).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
