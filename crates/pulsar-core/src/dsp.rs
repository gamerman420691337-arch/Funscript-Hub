//! Bounded reference motion kernels, independent of media decoding and models.
//! Global vertical translation is an explicitly limited baseline, not semantic
//! target tracking or a qualified optical-flow/model implementation.

use crate::{CoreError, NormalizedPosition};

pub const MAX_ANALYSIS_PIXELS: usize = 1_048_576;
pub const MAX_VERTICAL_SEARCH: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerticalMotionEstimate {
    /// Positive means content moved down in image coordinates.
    pub displacement_pixels: f64,
    /// An uncalibrated uniqueness score, not a probability of correctness.
    pub confidence: f64,
    /// False for textureless or tied/ambiguous estimates. Consumers must keep
    /// missing evidence distinct from observed zero movement.
    pub has_evidence: bool,
}

/// Match grayscale frames using a fixed-overlap vertical SAD search. Every
/// candidate scores the same pixels, so a larger shift cannot win merely by
/// discarding more image rows. The search is bounded before any pixel access.
pub fn estimate_vertical_translation(
    previous: &[u8],
    current: &[u8],
    width: u32,
    height: u32,
    max_shift: u32,
) -> Result<VerticalMotionEstimate, CoreError> {
    let width = usize::try_from(width).map_err(|_| CoreError::Overflow)?;
    let height = usize::try_from(height).map_err(|_| CoreError::Overflow)?;
    let shift = usize::try_from(max_shift).map_err(|_| CoreError::Overflow)?;
    let pixels = width.checked_mul(height).ok_or(CoreError::Overflow)?;
    if width == 0
        || height == 0
        || pixels > MAX_ANALYSIS_PIXELS
        || previous.len() != pixels
        || current.len() != pixels
    {
        return Err(CoreError::InvalidGeometry(
            "grayscale dimensions must match bounded nonempty buffers".into(),
        ));
    }
    if max_shift == 0
        || max_shift > MAX_VERTICAL_SEARCH
        || shift.checked_mul(2).ok_or(CoreError::Overflow)? >= height
    {
        return Err(CoreError::InvalidGeometry(
            "vertical search must be 1..=32 and leave an interior image region".into(),
        ));
    }
    let min = *previous
        .iter()
        .min()
        .ok_or_else(|| CoreError::InvalidGeometry("empty grayscale buffer".into()))?;
    let max = *previous
        .iter()
        .max()
        .ok_or_else(|| CoreError::InvalidGeometry("empty grayscale buffer".into()))?;
    if min == max {
        return Ok(VerticalMotionEstimate {
            displacement_pixels: 0.0,
            confidence: 0.0,
            has_evidence: false,
        });
    }
    let mut best = u64::MAX;
    let mut second = u64::MAX;
    let mut best_shift = 0_i32;
    for dy in -(max_shift as i32)..=(max_shift as i32) {
        let mut error = 0_u64;
        for y in shift..height - shift {
            let other_y = (y as i64 + i64::from(dy)) as usize;
            for x in 0..width {
                error += u64::from(previous[y * width + x].abs_diff(current[other_y * width + x]));
            }
        }
        if error < best {
            second = best;
            best = error;
            best_shift = dy;
        } else if error == best {
            second = best;
            if dy.unsigned_abs() < best_shift.unsigned_abs() {
                best_shift = dy;
            }
        } else if error < second {
            second = error;
        }
    }
    let has_evidence = second > best;
    let confidence = if has_evidence {
        (second - best) as f64 / (second as f64 + 1.0)
    } else {
        0.0
    };
    Ok(VerticalMotionEstimate {
        displacement_pixels: if has_evidence {
            f64::from(best_shift)
        } else {
            0.0
        },
        confidence,
        has_evidence,
    })
}

/// Integrate image-relative displacement without normalizing the accumulated
/// trace to full range. Clamping constrains amplitude and never retimes it.
/// The caller must not integrate an estimate with has_evidence == false.
pub fn integrate_vertical_motion(
    position: NormalizedPosition,
    displacement_pixels: f64,
    height: u32,
) -> Result<NormalizedPosition, CoreError> {
    if height == 0 {
        return Err(CoreError::InvalidGeometry(
            "image height must be nonzero".into(),
        ));
    }
    if !displacement_pixels.is_finite() {
        return Err(CoreError::NonFinite);
    }
    let next = position.value() + displacement_pixels / f64::from(height);
    if !next.is_finite() {
        return Err(CoreError::NonFinite);
    }
    NormalizedPosition::new(next.clamp(0.0, 1.0))
}
