//! YOLO26 & RF-DETR NMS-Free Direct Tensor Decoding Engine.
//!
//! Provides zero-overhead, end-to-end direct query decoding for modern one-to-one matched
//! detectors (YOLO26, RF-DETR, D-FINE). Eliminates slow Non-Maximum Suppression (NMS) and
//! Distribution Focal Loss (DFL) de-quantization loops.

use crate::neural::yolo::CLASS_NAMES;
use serde::{Deserialize, Serialize};

/// High-performance NMS-free detection candidate
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectDetection {
    pub class_id: usize,
    pub class_name: &'static str,
    pub confidence: f32,
    /// Normalized coordinates [x1, y1, x2, y2] in [0.0, 1.0]
    pub bbox: [f32; 4],
    /// Normalized center point (cx, cy) in [0.0, 1.0]
    pub center: (f32, f32),
    /// Optional prototype mask coefficients (e.g. 32 coefficients for RF-DETR-Seg)
    pub mask_coeffs: Option<Vec<f32>>,
}

/// Tensor memory layout of the NMS-free model output
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectTensorLayout {
    /// Queries first: shape [1, num_queries, 4 + num_classes (+ num_masks)] (e.g. RF-DETR, YOLO26 standard export)
    QueryMajor,
    /// Channels first: shape [1, 4 + num_classes (+ num_masks), num_queries] (e.g. transposed ONNX export)
    ChannelMajor,
}

/// Decodes NMS-free one-to-one predictions directly into filtered detections.
///
/// Bypasses all pairwise IoU intersection calculations, yielding O(K) decoding latency.
#[allow(dead_code)]
pub fn decode_nms_free_detections(
    output_slice: &[f32],
    num_queries: usize,
    num_classes: usize,
    num_mask_coeffs: usize,
    layout: DirectTensorLayout,
    conf_threshold: f32,
) -> Vec<DirectDetection> {
    let query_stride = 4 + num_classes + num_mask_coeffs;
    if output_slice.len() < num_queries * query_stride {
        return Vec::new();
    }

    let mut detections = Vec::with_capacity(num_queries.min(64));

    for q in 0..num_queries {
        // 1. Extract coordinates [cx, cy, w, h] or [x1, y1, x2, y2]
        let (raw_x1, raw_y1, raw_x2, raw_y2) = match layout {
            DirectTensorLayout::QueryMajor => {
                let base = q * query_stride;
                (
                    output_slice[base],
                    output_slice[base + 1],
                    output_slice[base + 2],
                    output_slice[base + 3],
                )
            }
            DirectTensorLayout::ChannelMajor => (
                output_slice[q],
                output_slice[num_queries + q],
                output_slice[2 * num_queries + q],
                output_slice[3 * num_queries + q],
            ),
        };

        // Standardize to normalized bounding box [x1, y1, x2, y2]
        // If raw coordinates are in (cx, cy, w, h) center format:
        let (x1, y1, x2, y2) = if raw_x2 < raw_x1 || raw_y2 < raw_y1 {
            // Unlikely or corrupted; clamp
            (raw_x1.clamp(0.0, 1.0), raw_y1.clamp(0.0, 1.0), raw_x2.clamp(0.0, 1.0), raw_y2.clamp(0.0, 1.0))
        } else if raw_x2 <= 1.0 && raw_y2 <= 1.0 && raw_x1 >= 0.0 && raw_y1 >= 0.0 && (raw_x2 - raw_x1) >= 0.0 {
            // Already in [x1, y1, x2, y2] format
            (raw_x1, raw_y1, raw_x2, raw_y2)
        } else {
            // Treat as cx, cy, w, h
            let cx = raw_x1;
            let cy = raw_y1;
            let w = raw_x2;
            let h = raw_y2;
            (
                (cx - w * 0.5).clamp(0.0, 1.0),
                (cy - h * 0.5).clamp(0.0, 1.0),
                (cx + w * 0.5).clamp(0.0, 1.0),
                (cy + h * 0.5).clamp(0.0, 1.0),
            )
        };

        // 2. Find best class score
        let mut best_class = 0;
        let mut best_score = 0.0f32;

        for c in 0..num_classes {
            let score = match layout {
                DirectTensorLayout::QueryMajor => {
                    let idx = q * query_stride + 4 + c;
                    output_slice[idx]
                }
                DirectTensorLayout::ChannelMajor => {
                    let idx = (4 + c) * num_queries + q;
                    output_slice[idx]
                }
            };

            // Modern direct decoders output probabilities in [0.0, 1.0] or logits; handle both
            let prob = if score > 1.0 || score < 0.0 {
                // Sigmoidal logit conversion
                1.0 / (1.0 + (-score).exp())
            } else {
                score
            };

            if prob > best_score {
                best_score = prob;
                best_class = c;
            }
        }

        if best_score < conf_threshold {
            continue;
        }

        // 3. Extract mask coefficients if this is an instance segmentation model (RF-DETR-Seg)
        let mask_coeffs = if num_mask_coeffs > 0 {
            let mut coeffs = Vec::with_capacity(num_mask_coeffs);
            for m in 0..num_mask_coeffs {
                let coeff = match layout {
                    DirectTensorLayout::QueryMajor => {
                        let idx = q * query_stride + 4 + num_classes + m;
                        output_slice[idx]
                    }
                    DirectTensorLayout::ChannelMajor => {
                        let idx = (4 + num_classes + m) * num_queries + q;
                        output_slice[idx]
                    }
                };
                coeffs.push(coeff);
            }
            Some(coeffs)
        } else {
            None
        };

        let cx = (x1 + x2) * 0.5;
        let cy = (y1 + y2) * 0.5;
        let class_name = if best_class < CLASS_NAMES.len() {
            CLASS_NAMES[best_class]
        } else {
            "unknown"
        };

        detections.push(DirectDetection {
            class_id: best_class,
            class_name,
            confidence: best_score,
            bbox: [x1, y1, x2, y2],
            center: (cx, cy),
            mask_coeffs,
        });
    }

    // Sort by confidence descending
    detections.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
    detections
}

/// Generates a binary or grayscale mask by computing the matrix product of mask coefficients and prototypes
///
/// coefficients: [num_masks] (e.g. 32)
/// proto_masks: [num_masks, proto_h, proto_w] (e.g. [32, 160, 160])
#[allow(dead_code)]
pub fn assemble_instance_mask(
    coeffs: &[f32],
    proto_masks: &[f32],
    proto_w: usize,
    proto_h: usize,
    bbox: [f32; 4],
) -> Vec<u8> {
    let num_proto = coeffs.len();
    let spatial_dim = proto_w * proto_h;
    if proto_masks.len() < num_proto * spatial_dim {
        return vec![0u8; spatial_dim];
    }

    let mut mask = vec![0u8; spatial_dim];

    let bx1 = (bbox[0] * proto_w as f32) as usize;
    let by1 = (bbox[1] * proto_h as f32) as usize;
    let bx2 = (bbox[2] * proto_w as f32).ceil() as usize;
    let by2 = (bbox[3] * proto_h as f32).ceil() as usize;

    let bx2 = bx2.min(proto_w);
    let by2 = by2.min(proto_h);

    for y in by1..by2 {
        let row_offset = y * proto_w;
        for x in bx1..bx2 {
            let pixel_idx = row_offset + x;
            let mut sum = 0.0f32;

            for m in 0..num_proto {
                let proto_val = proto_masks[m * spatial_dim + pixel_idx];
                sum += coeffs[m] * proto_val;
            }

            // Sigmoid activation
            let activated = 1.0 / (1.0 + (-sum).exp());
            mask[pixel_idx] = (activated * 255.0) as u8;
        }
    }

    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yolo26_nms_free_decoding_query_major() {
        let num_queries = 3;
        let num_classes = 10;
        let num_masks = 0;
        let stride = 4 + num_classes; // 14

        let mut data = vec![0.0f32; num_queries * stride];

        // Query 0: center (0.5, 0.5), w=0.2, h=0.4, class 0 ("penis") confidence 0.92
        data[0] = 0.4; // x1
        data[1] = 0.3; // y1
        data[2] = 0.6; // x2
        data[3] = 0.7; // y2
        data[4 + 0] = 0.92;

        // Query 1: low confidence (0.15) - should be filtered
        data[stride] = 0.1;
        data[stride + 1] = 0.1;
        data[stride + 2] = 0.3;
        data[stride + 3] = 0.3;
        data[stride + 4 + 2] = 0.15;

        // Query 2: class 2 ("pussy") confidence 0.85
        let base2 = 2 * stride;
        data[base2] = 0.2;
        data[base2 + 1] = 0.2;
        data[base2 + 2] = 0.5;
        data[base2 + 3] = 0.5;
        data[base2 + 4 + 2] = 0.85;

        let detections = decode_nms_free_detections(
            &data,
            num_queries,
            num_classes,
            num_masks,
            DirectTensorLayout::QueryMajor,
            0.30,
        );

        assert_eq!(detections.len(), 2);
        assert_eq!(detections[0].class_id, 0);
        assert_eq!(detections[0].class_name, "penis");
        assert!((detections[0].confidence - 0.92).abs() < 1e-4);
        assert_eq!(detections[0].bbox, [0.4, 0.3, 0.6, 0.7]);
        assert_eq!(detections[0].center, (0.5, 0.5));

        assert_eq!(detections[1].class_id, 2);
        assert_eq!(detections[1].class_name, "pussy");
        assert!((detections[1].confidence - 0.85).abs() < 1e-4);
    }

    #[test]
    fn test_yolo26_nms_free_decoding_channel_major() {
        let num_queries = 2;
        let num_classes = 4;
        let num_masks = 0;
        let channels = 4 + num_classes; // 8

        let mut data = vec![0.0f32; channels * num_queries];

        // Query 0: x1=0.2, y1=0.2, x2=0.4, y2=0.4, class 1 conf 0.88
        data[0] = 0.2; // ch 0 (x1)
        data[num_queries + 0] = 0.2; // ch 1 (y1)
        data[2 * num_queries + 0] = 0.4; // ch 2 (x2)
        data[3 * num_queries + 0] = 0.4; // ch 3 (y2)
        data[(4 + 1) * num_queries + 0] = 0.88; // ch 5 (class 1)

        let detections = decode_nms_free_detections(
            &data,
            num_queries,
            num_classes,
            num_masks,
            DirectTensorLayout::ChannelMajor,
            0.50,
        );

        assert_eq!(detections.len(), 1);
        assert_eq!(detections[0].class_id, 1);
        assert!((detections[0].confidence - 0.88).abs() < 1e-4);
    }

    #[test]
    fn test_assemble_instance_mask_linear_combination() {
        let proto_w = 4;
        let proto_h = 4;
        let coeffs = vec![1.0, -1.0];
        let mut proto_masks = vec![0.0f32; 2 * proto_w * proto_h];

        // Proto 0 has 2.0 everywhere, Proto 1 has 1.0 everywhere
        for i in 0..16 {
            proto_masks[i] = 2.0;
            proto_masks[16 + i] = 1.0;
        }

        // Sum = 1.0 * 2.0 + (-1.0) * 1.0 = 1.0
        // Sigmoid(1.0) = 1 / (1 + exp(-1.0)) = 0.731
        let mask = assemble_instance_mask(&coeffs, &proto_masks, proto_w, proto_h, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(mask.len(), 16);
        let expected_val = (0.7310586 * 255.0) as u8;
        assert!((mask[0] as i32 - expected_val as i32).abs() <= 1);
    }
}
