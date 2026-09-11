//! ONNX effect adapter. No caller outside the worker may create a session.
//!
//! This adapter accepts one declared tensor contract rather than guessing a
//! decoder from tensor lengths. Output geometry is validated before it crosses
//! into the checked observation domain.

use anyhow::{bail, Context, Result};
use ndarray::Array4;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use pulsar_protocol::{DetectorDecoder, ModelInputContract, TensorLayout};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct RawDetection {
    pub class_id: u32,
    pub score: f32,
    pub bbox: [f32; 4],
}

pub struct Detector {
    session: Session,
    contract: ModelInputContract,
}

impl Detector {
    pub fn new(
        model: &Path,
        runtime: &Path,
        contract: &ModelInputContract,
        threads: u16,
    ) -> Result<Self> {
        if contract.channels != 3
            || !matches!(contract.layout, TensorLayout::Nchw)
            || !matches!(contract.decoder, DetectorDecoder::YoloChannelMajor)
            || contract.width == 0
            || contract.height == 0
            || contract.width > 1024
            || contract.height > 1024
            || contract.class_count == 0
            || contract.class_count > 256
        {
            bail!("unsupported model contract: require bounded RGB NCHW YOLO channel-major input/output");
        }
        let committed = ort::init_from(runtime)
            .map_err(|error| anyhow::anyhow!("failed to load the pinned ONNX runtime: {error}"))?
            .with_name("pulsar-isolated-worker")
            .commit();
        if !committed {
            bail!("ONNX runtime was already initialized; a new attempt process is required");
        }
        let session = Session::builder()
            .map_err(|error| anyhow::anyhow!("failed to create CPU session: {error}"))?
            .with_intra_threads(usize::from(threads))
            .map_err(|error| anyhow::anyhow!("failed to set inference thread budget: {error}"))?
            .with_inter_threads(1)
            .map_err(|error| anyhow::anyhow!("failed to set inter-op thread budget: {error}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| anyhow::anyhow!("failed to configure CPU inference: {error}"))?
            .commit_from_file(model)
            .map_err(|error| anyhow::anyhow!("failed to load the pinned model: {error}"))?;
        Ok(Self {
            session,
            contract: contract.clone(),
        })
    }

    pub fn detect(&mut self, rgb: &[u8], width: u32, height: u32) -> Result<Vec<RawDetection>> {
        let expected = super::media::frame_bytes(width, height)?;
        if rgb.len() != expected {
            bail!("RGB tensor source violates its declared layout");
        }
        let w = self.contract.width as usize;
        let h = self.contract.height as usize;
        let mut input = Array4::<f32>::zeros((1, 3, h, w));
        for y in 0..h {
            let source_y = y * height as usize / h;
            for x in 0..w {
                let source_x = x * width as usize / w;
                let base = (source_y * width as usize + source_x) * 3;
                for c in 0..3 {
                    input[[0, c, y, x]] = rgb[base + c] as f32 / 255.0;
                }
            }
        }
        let tensor = Tensor::from_array(input)
            .map_err(|error| anyhow::anyhow!("input tensor creation failed: {error}"))?;
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .map_err(|error| anyhow::anyhow!("CPU inference failed: {error}"))?;
        if outputs.len() != 1 {
            bail!("model contract requires exactly one detection output");
        }
        let (_, output) = outputs
            .into_iter()
            .next()
            .context("model returned no tensor")?;
        let (shape, values) = output
            .try_extract_tensor::<f32>()
            .map_err(|error| anyhow::anyhow!("model output must be float32: {error}"))?;
        decode_channel_major(
            values,
            shape.as_ref(),
            self.contract.class_count as usize,
            self.contract.width,
            self.contract.height,
            0.25,
        )
    }
}

fn decode_channel_major(
    values: &[f32],
    shape: &[i64],
    classes: usize,
    input_width: u32,
    input_height: u32,
    threshold: f32,
) -> Result<Vec<RawDetection>> {
    use pulsar_core::perception::{decode_detections, DetectorLayout, DetectorTransform};
    let shape = shape
        .iter()
        .map(|dimension| usize::try_from(*dimension))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("negative or overflowing detector dimension")?;
    // The adapter currently stretches RGB into the declared model dimensions.
    // Canonicalization therefore uses that exact transform, not letterbox math.
    // The public legacy adapter shape remains normalized for existing preview.
    let transform =
        DetectorTransform::resize(input_width, input_height, input_width, input_height)?;
    let detections = decode_detections(
        values,
        &shape,
        DetectorLayout::YoloChannelMajor,
        classes,
        &transform,
        f64::from(threshold),
        0.45,
    )?;
    Ok(detections
        .into_iter()
        .map(|detection| {
            let bounds = detection.bounds();
            RawDetection {
                class_id: detection.class_id(),
                score: detection.score() as f32,
                bbox: [
                    (bounds.x_min() / f64::from(input_width)) as f32,
                    (bounds.y_min() / f64::from(input_height)) as f32,
                    (bounds.x_max() / f64::from(input_width)) as f32,
                    (bounds.y_max() / f64::from(input_height)) as f32,
                ],
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_rejects_shape_guessing_nonfinite_and_reversed_boxes() {
        let good = [320.0, 320.0, 100.0, 200.0, 0.9];
        assert_eq!(
            decode_channel_major(&good, &[1, 5, 1], 1, 640, 640, 0.25)
                .unwrap()
                .len(),
            1
        );
        assert!(decode_channel_major(&good, &[1, 1, 5], 1, 640, 640, 0.25).is_err());
        let mut invalid = good;
        invalid[4] = f32::INFINITY;
        assert!(decode_channel_major(&invalid, &[1, 5, 1], 1, 640, 640, 0.25).is_err());
        invalid = good;
        invalid[2] = -5.0;
        assert!(decode_channel_major(&invalid, &[1, 5, 1], 1, 640, 640, 0.25).is_err());
    }

    #[test]
    fn clipping_remains_normalized_and_absence_is_empty() {
        let border = [0.0, 320.0, 100.0, 200.0, 0.9];
        let result = decode_channel_major(&border, &[1, 5, 1], 1, 640, 640, 0.25).unwrap();
        assert_eq!(result[0].bbox[0], 0.0);
        let absent = [320.0, 320.0, 100.0, 200.0, 0.01];
        assert!(decode_channel_major(&absent, &[1, 5, 1], 1, 640, 640, 0.25)
            .unwrap()
            .is_empty());
    }
}
