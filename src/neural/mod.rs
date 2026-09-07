//! Neural inference engine powered by ONNX Runtime (`ort`).

pub mod pose;
pub mod tracker;
pub mod yolo;

pub use pose::Pose3D;

use anyhow::{Context, Result};
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use yolo::{decode_yolo_detections, preprocess_image_rgb, Detection};

pub struct NeuralDetector {
    session: Session,
    pub conf_threshold: f32,
    pub iou_threshold: f32,
}

impl NeuralDetector {
    pub fn new<P: AsRef<Path>>(model_path: P, conf_threshold: f32) -> Result<Self> {
        let path_buf = model_path.as_ref().to_path_buf();
        let builder = Session::builder()
            .map_err(|e| anyhow::anyhow!("Failed to initialize SessionBuilder: {e}"))?;

        let session = builder
            .with_intra_threads(4)
            .map_err(|e| anyhow::anyhow!("Failed to configure thread count: {e}"))?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("Failed to configure optimization level: {e}"))?
            .commit_from_file(&path_buf)
            .map_err(|e| anyhow::anyhow!("Failed to load ONNX model from {path_buf:?}: {e}"))?;

        Ok(Self {
            session,
            conf_threshold,
            iou_threshold: 0.45,
        })
    }

    /// Run YOLO detection on raw RGB image bytes
    pub fn detect(
        &mut self,
        rgb_data: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Vec<Detection>> {
        let input_array: Array4<f32> = preprocess_image_rgb(rgb_data, width, height, 640);
        let input_tensor = Tensor::from_array(input_array)
            .map_err(|e| anyhow::anyhow!("Failed to create ONNX input tensor: {e}"))?;

        let inputs = ort::inputs![input_tensor];
        let outputs = self
            .session
            .run(inputs)
            .map_err(|e| anyhow::anyhow!("Inference execution failed: {e}"))?;

        // Extract output0 tensor (shape [1, 14, 8400])
        let (_, output_tensor) = outputs
            .into_iter()
            .next()
            .context("No output tensors returned from ONNX session")?;

        let (_shape, slice) = output_tensor
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Failed to extract output tensor: {e}"))?;

        let detections = decode_yolo_detections(
            slice,
            10,   // 10 anatomical classes
            8400, // 8400 anchor candidates
            self.conf_threshold,
            self.iou_threshold,
        );

        Ok(detections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_neural_detector_inference_with_model() {
        let model_path = Path::new("/home/brianklam/Desktop/Funscripts/FunGen/fungen-data/cache/models/FunGen-12n-pov-1.1.0.onnx");
        if !model_path.exists() {
            eprintln!("Model not found at {:?}, skipping live model test", model_path);
            return;
        }

        let mut detector = NeuralDetector::new(model_path, 0.25).expect("Failed to create NeuralDetector");
        let dummy_rgb = vec![128u8; 640 * 640 * 3];
        let detections = detector.detect(&dummy_rgb, 640, 640).expect("Inference failed");
        // Detections should execute successfully (may be empty or contain low-conf detections for a flat gray frame)
        println!("Live model inference succeeded, returned {} detections", detections.len());
    }

    #[test]
    fn test_detect_on_fixture_video() {
        let model_path = Path::new("/home/brianklam/Desktop/Funscripts/FunGen/fungen-data/cache/models/FunGen-12n-pov-1.1.0.onnx");
        let video_path = Path::new("/home/brianklam/Desktop/Funscripts/Testing Fixtures/handonly_sync_animation.mp4");
        if !model_path.exists() || !video_path.exists() {
            eprintln!("Fixture or model missing");
            return;
        }

        let mut detector = NeuralDetector::new(model_path, 0.01).expect("Failed to load model");
        for (i, input) in detector.session.inputs().iter().enumerate() {
            println!("Session input {}: name={}, type={:?}", i, input.name(), input.dtype());
        }
        for (i, output) in detector.session.outputs().iter().enumerate() {
            println!("Session output {}: name={}, type={:?}", i, output.name(), output.dtype());
        }

        let rgb = crate::video::extract_frame_at(video_path, 1000, 640, 640).expect("Failed to extract frame");
        println!("Sample rgb byte values: {:?}", &rgb[0..15]);

        let input_array: Array4<f32> = preprocess_image_rgb(&rgb, 640, 640, 640);
        let input_tensor = Tensor::from_array(input_array).unwrap();
        let inputs = ort::inputs![input_tensor];
        let outputs = detector.session.run(inputs).unwrap();
        for (name, out) in outputs {
            let (shape, slice) = out.try_extract_tensor::<f32>().unwrap();
            println!("Output {}: shape={:?}, total_elements={}", name, shape, slice.len());
            for c in 0..14 {
                let channel_slice = &slice[c * 8400..(c + 1) * 8400];
                let min_val = channel_slice.iter().fold(f32::INFINITY, |a, &b| a.min(b));
                let max_val = channel_slice.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
                println!("Channel {}: min={:.4}, max={:.4}", c, min_val, max_val);
            }
        }
    }
}

