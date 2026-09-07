//! YOLOv12 / YOLOv8 ONNX detection pre-processing and post-processing.

use ndarray::Array4;

pub const CLASS_NAMES: [&str; 10] = [
    "penis",   // 0
    "glans",   // 1
    "pussy",   // 2
    "butt",    // 3
    "anus",    // 4
    "breast",  // 5
    "navel",   // 6
    "hand",    // 7
    "face",    // 8
    "foot",    // 9
];

#[derive(Debug, Clone)]
pub struct Detection {
    pub class_id: usize,
    #[allow(dead_code)]
    pub class_name: &'static str,
    pub confidence: f32,
    /// Normalized coordinates [x1, y1, x2, y2] in [0.0, 1.0]
    pub bbox: [f32; 4],
    /// Normalized center point (cx, cy) in [0.0, 1.0]
    pub center: (f32, f32),
}

/// Preprocesses raw RGB image bytes into a 1x3x640x640 float32 tensor
pub fn preprocess_image_rgb(
    rgb_data: &[u8],
    src_width: usize,
    src_height: usize,
    target_dim: usize,
) -> Array4<f32> {
    let mut tensor = Array4::<f32>::zeros((1, 3, target_dim, target_dim));
    let plane_stride = target_dim * target_dim;
    // Safety: ndarray guarantees contiguous standard layout for zeros()
    let flat = tensor.as_slice_mut().unwrap();

    let x_scale = (src_width as f32) / (target_dim as f32);
    let y_scale = (src_height as f32) / (target_dim as f32);

    // Pre-compute coordinate lookup tables — eliminates float math from the inner loop
    let sx_lut: Vec<usize> = (0..target_dim)
        .map(|tx| ((tx as f32) * x_scale).clamp(0.0, (src_width - 1) as f32) as usize)
        .collect();
    let sy_lut: Vec<usize> = (0..target_dim)
        .map(|ty| ((ty as f32) * y_scale).clamp(0.0, (src_height - 1) as f32) as usize)
        .collect();

    const INV_255: f32 = 1.0 / 255.0;

    for ty in 0..target_dim {
        let sy = sy_lut[ty];
        let row_base = sy * src_width;
        let out_row = ty * target_dim;
        for tx in 0..target_dim {
            let src_idx = (row_base + sx_lut[tx]) * 3;
            // Write directly to flat slice with stride arithmetic — bypasses ndarray 4D indexing
            flat[out_row + tx] = (rgb_data[src_idx] as f32) * INV_255;
            flat[plane_stride + out_row + tx] = (rgb_data[src_idx + 1] as f32) * INV_255;
            flat[2 * plane_stride + out_row + tx] = (rgb_data[src_idx + 2] as f32) * INV_255;
        }
    }

    tensor
}

/// Decodes YOLO output tensor [1, 14, 8400] and applies Non-Maximum Suppression (NMS)
pub fn decode_yolo_detections(
    output_slice: &[f32],
    num_classes: usize,
    num_anchors: usize,
    conf_threshold: f32,
    iou_threshold: f32,
) -> Vec<Detection> {
    let num_channels = 4 + num_classes; // 14
    if output_slice.len() < num_channels * num_anchors {
        return Vec::new();
    }

    let mut candidates: Vec<Detection> = Vec::new();

    for i in 0..num_anchors {
        // Find best class score for this anchor
        let mut best_class = 0;
        let mut best_score = 0.0f32;

        for c in 0..num_classes {
            // output is in channel-major or transposed format: [channel, anchor]
            let score = output_slice[(4 + c) * num_anchors + i];
            if score > best_score {
                best_score = score;
                best_class = c;
            }
        }

        if best_score >= conf_threshold {
            let cx = output_slice[i] / 640.0;
            let cy = output_slice[num_anchors + i] / 640.0;
            let w = output_slice[2 * num_anchors + i] / 640.0;
            let h = output_slice[3 * num_anchors + i] / 640.0;

            let x1 = (cx - w * 0.5).clamp(0.0, 1.0);
            let y1 = (cy - h * 0.5).clamp(0.0, 1.0);
            let x2 = (cx + w * 0.5).clamp(0.0, 1.0);
            let y2 = (cy + h * 0.5).clamp(0.0, 1.0);

            candidates.push(Detection {
                class_id: best_class,
                class_name: CLASS_NAMES[best_class],
                confidence: best_score,
                bbox: [x1, y1, x2, y2],
                center: (cx.clamp(0.0, 1.0), cy.clamp(0.0, 1.0)),
            });
        }
    }

    // Sort by confidence descending
    candidates.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());

    // Apply Non-Maximum Suppression (NMS)
    let mut selected: Vec<Detection> = Vec::new();
    let mut suppressed = vec![false; candidates.len()];

    for i in 0..candidates.len() {
        if suppressed[i] {
            continue;
        }
        let current = &candidates[i];
        selected.push(current.clone());

        for j in (i + 1)..candidates.len() {
            if suppressed[j] || candidates[j].class_id != current.class_id {
                continue;
            }
            if calculate_iou(&current.bbox, &candidates[j].bbox) > iou_threshold {
                suppressed[j] = true;
            }
        }
    }

    selected
}

/// Calculate Intersection over Union (IoU) between two bounding boxes [x1, y1, x2, y2]
pub fn calculate_iou(box_a: &[f32; 4], box_b: &[f32; 4]) -> f32 {
    let x_left = box_a[0].max(box_b[0]);
    let y_top = box_a[1].max(box_b[1]);
    let x_right = box_a[2].min(box_b[2]);
    let y_bottom = box_a[3].min(box_b[3]);

    if x_right < x_left || y_bottom < y_top {
        return 0.0;
    }

    let intersection_area = (x_right - x_left) * (y_bottom - y_top);
    let area_a = (box_a[2] - box_a[0]) * (box_a[3] - box_a[1]);
    let area_b = (box_b[2] - box_b[0]) * (box_b[3] - box_b[1]);
    let union_area = area_a + area_b - intersection_area;

    if union_area > 0.0 {
        intersection_area / union_area
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preprocess_dimensions_and_normalization() {
        let src_w = 128;
        let src_h = 128;
        let rgb_data = vec![255u8; src_w * src_h * 3];
        let tensor = preprocess_image_rgb(&rgb_data, src_w, src_h, 640);

        assert_eq!(tensor.shape(), &[1, 3, 640, 640]);
        // All pixels should be normalized to 1.0
        assert!((tensor[[0, 0, 320, 320]] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_iou_calculation() {
        let box1 = [0.0, 0.0, 0.5, 0.5];
        let box2 = [0.0, 0.0, 0.5, 0.5];
        assert!((calculate_iou(&box1, &box2) - 1.0).abs() < 1e-4);

        let box3 = [0.6, 0.6, 1.0, 1.0];
        assert_eq!(calculate_iou(&box1, &box3), 0.0);
    }
}
