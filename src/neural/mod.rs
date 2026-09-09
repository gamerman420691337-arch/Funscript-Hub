//! Neural inference engine powered by ONNX Runtime (`ort`).

pub mod hypothesis;
pub mod model_manager;
pub mod pipeline;
pub mod point_tracker;
pub mod pose;
pub mod router;
pub mod sam;
pub mod tracker;
pub mod yolo;
pub mod yolo26;

#[allow(unused_imports)]
pub use hypothesis::{HypothesisEngine, ObservabilityState, ReconciledFrame, TrajectoryHypothesis};
#[allow(unused_imports)]
pub use model_manager::{
    auto_detect_model, default_model_cache_dir, install_or_download_default_model, model_registry,
    ModelInfo, ModelRegistryEntry, ModelType, DEFAULT_MODEL_NAME,
};
#[allow(unused_imports)]
pub use pipeline::{AdaptiveFrameOutput, AdaptiveMotionPipeline, AdaptiveProfile};
#[allow(unused_imports)]
pub use point_tracker::{PointTrajectory, SurfaceKinematics, TapPointTracker, TapTrackerConfig};
pub use pose::Pose3D;
#[allow(unused_imports)]
pub use router::{
    CalibratedUncertainty, ConfidenceMetrics, ExecutionBranch, FailureRouter, RouterConfig,
    RouterStats,
};
#[allow(unused_imports)]
pub use yolo26::{
    assemble_instance_mask, decode_nms_free_detections, DirectDetection, DirectTensorLayout,
    Yolo26Config, Yolo26Detector,
};

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

    /// Run NMS-Free direct head detection (YOLO26 / RF-DETR)
    #[allow(dead_code)]
    pub fn detect_direct(
        &mut self,
        rgb_data: &[u8],
        width: usize,
        height: usize,
        num_classes: usize,
        num_masks: usize,
        layout: DirectTensorLayout,
    ) -> Result<Vec<DirectDetection>> {
        let input_array: Array4<f32> = preprocess_image_rgb(rgb_data, width, height, 640);
        let input_tensor = Tensor::from_array(input_array)
            .map_err(|e| anyhow::anyhow!("Failed to create ONNX input tensor: {e}"))?;

        let inputs = ort::inputs![input_tensor];
        let outputs = self
            .session
            .run(inputs)
            .map_err(|e| anyhow::anyhow!("Inference execution failed: {e}"))?;

        let (_, output_tensor) = outputs
            .into_iter()
            .next()
            .context("No output tensors returned from ONNX session")?;

        let (shape, slice) = output_tensor
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Failed to extract output tensor: {e}"))?;

        // Determine query count from tensor shape
        let num_queries = match layout {
            DirectTensorLayout::QueryMajor => {
                if shape.len() >= 2 { shape[shape.len() - 2] as usize } else { 300 }
            }
            DirectTensorLayout::ChannelMajor => {
                if shape.len() >= 1 { shape[shape.len() - 1] as usize } else { 300 }
            }
        };

        let detections = decode_nms_free_detections(
            slice,
            num_queries,
            num_classes,
            num_masks,
            layout,
            self.conf_threshold,
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
        let input_array: Array4<f32> = preprocess_image_rgb(&rgb, 640, 640, 640);
        let input_tensor = Tensor::from_array(input_array).unwrap();
        let inputs = ort::inputs![input_tensor];
        let outputs = detector.session.run(inputs).unwrap();
        for (name, out) in outputs {
            let (shape, slice) = out.try_extract_tensor::<f32>().unwrap();
            println!("Output {}: shape={:?}, total_elements={}", name, shape, slice.len());
        }
    }
}
