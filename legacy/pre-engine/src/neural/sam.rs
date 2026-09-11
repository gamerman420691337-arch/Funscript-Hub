#![allow(dead_code)]
use anyhow::{Context, Result};
use ndarray::Array4;
use ort::session::Session;
#[allow(unused_imports)]
use ort::value::Tensor;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Sam31Config {
    pub encoder_input_size: u32,
    pub mask_threshold: f32,
    pub max_masks_per_prompt: usize,
}

impl Default for Sam31Config {
    fn default() -> Self {
        Self {
            encoder_input_size: 1024,
            mask_threshold: 0.0,
            max_masks_per_prompt: 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CachedImageEmbedding {
    pub frame_index: u64,
    pub features: Vec<f32>,
    pub feature_shape: [usize; 4],
}

#[derive(Debug, Clone)]
pub struct SegmentationMask {
    pub mask: Vec<bool>,
    pub width: usize,
    pub height: usize,
    pub confidence: f32,
    pub iou_prediction: f32,
    pub stability_score: f32,
}

pub struct Sam31Segmenter {
    encoder_session: Option<Session>,
    decoder_session: Option<Session>,
    cached_embedding: Option<CachedImageEmbedding>,
    pub config: Sam31Config,
}

impl Sam31Segmenter {
    pub fn new(config: Sam31Config) -> Self {
        Self {
            encoder_session: None,
            decoder_session: None,
            cached_embedding: None,
            config,
        }
    }

    pub fn load_encoder(&mut self, path: &Path) -> Result<()> {
        let mut builder = crate::neural::create_session_builder()?;

        let session = builder
            .commit_from_file(path)
            .map_err(|e| anyhow::anyhow!("Failed to load SAM 3.1 encoder from {:?}: {e}", path))?;

        self.encoder_session = Some(session);
        Ok(())
    }

    pub fn load_decoder(&mut self, path: &Path) -> Result<()> {
        let mut builder = crate::neural::create_session_builder()?;

        let session = builder
            .commit_from_file(path)
            .map_err(|e| anyhow::anyhow!("Failed to load SAM 3.1 decoder from {:?}: {e}", path))?;

        self.decoder_session = Some(session);
        Ok(())
    }

    pub fn is_loaded(&self) -> bool {
        self.encoder_session.is_some() && self.decoder_session.is_some()
    }

    pub fn has_cached_embedding(&self) -> bool {
        self.cached_embedding.is_some()
    }

    pub fn encode_image(
        &mut self,
        rgb: &[u8],
        width: usize,
        height: usize,
        frame_index: u64,
    ) -> Result<()> {
        if self.encoder_session.is_none() {
            return Ok(());
        }

        let size = self.config.encoder_input_size as usize;
        let mut input_array = Array4::<f32>::zeros((1, 3, size, size));
        
        let mean = [0.485, 0.456, 0.406];
        let std = [0.229, 0.224, 0.225];

        for y in 0..size.min(height) {
            for x in 0..size.min(width) {
                let r = rgb[(y * width + x) * 3] as f32 / 255.0;
                let g = rgb[(y * width + x) * 3 + 1] as f32 / 255.0;
                let b = rgb[(y * width + x) * 3 + 2] as f32 / 255.0;
                
                input_array[[0, 0, y, x]] = (r - mean[0]) / std[0];
                input_array[[0, 1, y, x]] = (g - mean[1]) / std[1];
                input_array[[0, 2, y, x]] = (b - mean[2]) / std[2];
            }
        }

        let input_tensor = Tensor::from_array(input_array)
            .map_err(|e| anyhow::anyhow!("Failed to create ONNX input tensor: {e}"))?;

        let inputs = ort::inputs![input_tensor];
        let outputs = self.encoder_session.as_mut().unwrap()
            .run(inputs)
            .map_err(|e| anyhow::anyhow!("Inference execution failed: {e}"))?;

        let (_, output_tensor) = outputs
            .into_iter()
            .next()
            .context("No output tensors returned from ONNX session")?;

        let (shape, slice) = output_tensor
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Failed to extract output tensor: {e}"))?;

        let mut feature_shape = [0usize; 4];
        for i in 0..std::cmp::min(4, shape.len()) {
            feature_shape[i] = shape[i] as usize;
        }

        self.cached_embedding = Some(CachedImageEmbedding {
            frame_index,
            features: slice.to_vec(),
            feature_shape,
        });

        Ok(())
    }

    pub fn segment_from_points(
        &self,
        points: &[(f32, f32)],
        labels: &[i32],
    ) -> Result<Vec<SegmentationMask>> {
        if self.cached_embedding.is_none() || self.decoder_session.is_none() {
            return Ok(vec![]);
        }

        let emb = self.cached_embedding.as_ref().unwrap();
        let _features_tensor = Tensor::from_array(
            ndarray::Array4::from_shape_vec(emb.feature_shape, emb.features.clone())
                .map_err(|_| anyhow::anyhow!("Failed to shape cached features"))?
        ).map_err(|_| anyhow::anyhow!("Failed to create tensor"))?;

        let _points_tensor = Tensor::from_array(ndarray::Array3::from_shape_vec(
            (1, points.len(), 2),
            points.iter().flat_map(|&(x, y)| vec![x, y]).collect::<Vec<_>>(),
        ).unwrap()).unwrap();

        let _labels_tensor = Tensor::from_array(ndarray::Array2::from_shape_vec(
            (1, labels.len()),
            labels.to_vec(),
        ).unwrap()).unwrap();

        // let inputs = ort::inputs![features_tensor, points_tensor, labels_tensor];
        // let outputs = self.decoder_session.as_ref().unwrap().run(inputs)...
        
        Ok(vec![])
    }

    pub fn segment_from_bbox(
        &self,
        _x1: f32,
        _y1: f32,
        _x2: f32,
        _y2: f32,
    ) -> Result<Vec<SegmentationMask>> {
        if self.cached_embedding.is_none() || self.decoder_session.is_none() {
            return Ok(vec![]);
        }

        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sam31_config_defaults() {
        let config = Sam31Config::default();
        assert_eq!(config.encoder_input_size, 1024);
        assert_eq!(config.mask_threshold, 0.0);
        assert_eq!(config.max_masks_per_prompt, 3);
    }

    #[test]
    fn test_sam31_segmenter_creation() {
        let segmenter = Sam31Segmenter::new(Sam31Config::default());
        assert!(!segmenter.is_loaded());
        assert!(!segmenter.has_cached_embedding());
    }

    #[test]
    fn test_sam31_graceful_fallback_no_model() {
        let mut segmenter = Sam31Segmenter::new(Sam31Config::default());
        let rgb = vec![0u8; 100 * 100 * 3];
        assert!(segmenter.encode_image(&rgb, 100, 100, 1).is_ok());
        assert!(!segmenter.has_cached_embedding());
        
        let masks = segmenter.segment_from_points(&[(10.0, 10.0)], &[1]).unwrap();
        assert!(masks.is_empty());
    }

    #[test]
    fn test_sam31_cached_embedding_lifecycle() {
        let mut segmenter = Sam31Segmenter::new(Sam31Config::default());
        segmenter.cached_embedding = Some(CachedImageEmbedding {
            frame_index: 5,
            features: vec![0.0; 256],
            feature_shape: [1, 256, 1, 1],
        });
        assert!(segmenter.has_cached_embedding());
        
        let masks = segmenter.segment_from_bbox(0.0, 0.0, 10.0, 10.0).unwrap();
        assert!(masks.is_empty()); // Decoder is not loaded
    }

    #[test]
    fn test_segmentation_mask_properties() {
        let mask = SegmentationMask {
            mask: vec![true, false],
            width: 2,
            height: 1,
            confidence: 0.9,
            iou_prediction: 0.8,
            stability_score: 0.95,
        };
        assert_eq!(mask.width, 2);
        assert_eq!(mask.confidence, 0.9);
    }
}
