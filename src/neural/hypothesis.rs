//! Multi-Hypothesis Tracking & Bidirectional Temporal Reconciler.
//!
//! Maintains competing trajectory hypotheses across occlusion intervals and performs
//! Bayesian forward-backward temporal fusion. Explicitly separates Observed, Inferred,
//! and Unresolved motion states without blindly averaging conflicting tracks.

use serde::{Deserialize, Serialize};

/// Explicit observability classification of an estimated motion point
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservabilityState {
    /// Directly observed via high-confidence neural detection or consistent point tracks
    Observed,
    /// Inferred during occlusion via hypothesis selection, shaft extrapolation, or audio transients
    Inferred,
    /// Kinematically unobservable interval; wide calibrated uncertainty bounds
    Unresolved,
}

/// A candidate trajectory hypothesis over an ambiguous time window
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryHypothesis {
    pub hypothesis_id: usize,
    pub label: &'static str,
    /// Predicted coordinate sequence (timestamp_ms, pos)
    pub path: Vec<(i64, f32)>,
    /// Accumulated likelihood / log-probability score
    pub log_likelihood: f32,
    /// Estimated variance / uncertainty
    pub variance: f32,
    /// Cross-modal alignment score (e.g. aligned with audio transients)
    pub audio_alignment: f32,
}

#[allow(dead_code)]
impl TrajectoryHypothesis {
    pub fn new(hypothesis_id: usize, label: &'static str, initial_pos: f32, variance: f32) -> Self {
        Self {
            hypothesis_id,
            label,
            path: vec![(0, initial_pos)],
            log_likelihood: 0.0,
            variance: variance.max(1e-4),
            audio_alignment: 0.0,
        }
    }
}

/// Reconciled state for a single temporal frame
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReconciledFrame {
    pub timestamp_ms: i64,
    /// Reconciled position in [0.0, 100.0]
    pub position: f32,
    /// Calibrated uncertainty (+/- 1 sigma)
    pub sigma: f32,
    /// Observability classification
    pub observability: ObservabilityState,
    /// ID of the dominant winning hypothesis
    pub dominant_hypothesis_id: usize,
}

/// Dynamic hypothesis tree and bidirectional reconciler
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct HypothesisEngine {
    pub active_hypotheses: Vec<TrajectoryHypothesis>,
    next_id: usize,
}

#[allow(dead_code)]
impl HypothesisEngine {
    pub fn new() -> Self {
        Self {
            active_hypotheses: Vec::new(),
            next_id: 0,
        }
    }

    /// Reset engine
    pub fn reset(&mut self) {
        self.active_hypotheses.clear();
        self.next_id = 0;
    }

    /// Initialize competing hypotheses for an occlusion event (e.g. penetration vs reversal vs dwell)
    pub fn branch_occlusion_hypotheses(&mut self, current_pos: f32, velocity: f32, timestamp_ms: i64) {
        self.active_hypotheses.clear();

        // Hypothesis 0: Penetration Continuation (continues along approach vector)
        let mut h_cont = TrajectoryHypothesis::new(0, "continuation", current_pos, 25.0);
        h_cont.path.push((timestamp_ms, (current_pos + velocity * 10.0).clamp(0.0, 100.0)));
        h_cont.log_likelihood = 0.0;

        // Hypothesis 1: Immediate Turnaround / Reversal (impact bounce)
        let mut h_rev = TrajectoryHypothesis::new(1, "reversal", current_pos, 35.0);
        h_rev.path.push((timestamp_ms, (current_pos - velocity * 5.0).clamp(0.0, 100.0)));
        h_rev.log_likelihood = -0.5;

        // Hypothesis 2: Stationary Dwell / Contact Pause
        let mut h_dwell = TrajectoryHypothesis::new(2, "dwell", current_pos, 45.0);
        h_dwell.path.push((timestamp_ms, current_pos));
        h_dwell.log_likelihood = -1.0;

        self.active_hypotheses.push(h_cont);
        self.active_hypotheses.push(h_rev);
        self.active_hypotheses.push(h_dwell);
    }

    /// Score active hypotheses using cross-modal evidence (e.g. audio transient arrival)
    pub fn update_evidence(&mut self, timestamp_ms: i64, has_audio_transient: bool, visible_shaft_velocity: Option<f32>) {
        for h in &mut self.active_hypotheses {
            if has_audio_transient {
                // Audio transient strongly supports reversal/impact turnaround
                if h.label == "reversal" {
                    h.log_likelihood += 2.5;
                    h.audio_alignment += 1.0;
                } else if h.label == "continuation" {
                    h.log_likelihood -= 1.5;
                }
            }

            if let Some(shaft_v) = visible_shaft_velocity {
                // If the external shaft is moving in a given direction, evaluate path agreement
                let h_dir = if h.path.len() >= 2 {
                    let p1 = h.path[h.path.len() - 1].1;
                    let p0 = h.path[h.path.len() - 2].1;
                    p1 - p0
                } else {
                    0.0
                };

                if (h_dir > 0.0 && shaft_v > 0.0) || (h_dir < 0.0 && shaft_v < 0.0) {
                    h.log_likelihood += 1.2;
                } else {
                    h.log_likelihood -= 1.0;
                }
            }

            // Propagate variance over time
            h.variance += 2.0;
            // Record timestamp in path
            let last_p = h.path.last().map(|(_, p)| *p).unwrap_or(50.0);
            h.path.push((timestamp_ms, last_p));
        }
    }

    /// Select the highest-likelihood hypothesis
    pub fn select_best_hypothesis(&self) -> Option<&TrajectoryHypothesis> {
        self.active_hypotheses.iter().max_by(|a, b| {
            a.log_likelihood.partial_cmp(&b.log_likelihood).unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Bayesian Bidirectional Temporal Reconciliation
    ///
    /// Combines forward trajectory (fwd_pos, fwd_var) and backward trajectory (bwd_pos, bwd_var).
    /// If forward and backward predictions conflict beyond discrepancy_threshold, it does NOT
    /// average them blindly; it selects the more confident hypothesis or marks the interval unresolved.
    pub fn reconcile_bidirectional(
        fwd_pos: f32,
        fwd_var: f32,
        bwd_pos: f32,
        bwd_var: f32,
        discrepancy_threshold: f32,
        timestamp_ms: i64,
    ) -> ReconciledFrame {
        let diff = (fwd_pos - bwd_pos).abs();

        if diff <= discrepancy_threshold {
            // Forward and backward passes agree within tolerance: Bayesian inverse-variance fusion
            let inv_fwd = 1.0 / fwd_var.max(1e-4);
            let inv_bwd = 1.0 / bwd_var.max(1e-4);
            let total_inv = inv_fwd + inv_bwd;

            let fused_pos = (fwd_pos * inv_fwd + bwd_pos * inv_bwd) / total_inv;
            let fused_var = 1.0 / total_inv;
            let sigma = fused_var.sqrt();

            ReconciledFrame {
                timestamp_ms,
                position: fused_pos.clamp(0.0, 100.0),
                sigma,
                observability: if sigma < 8.0 {
                    ObservabilityState::Observed
                } else {
                    ObservabilityState::Inferred
                },
                dominant_hypothesis_id: 0,
            }
        } else {
            // Conflict detected: forward and backward passes diverge (e.g. identity swap, long occlusion)
            // Pick the branch with significantly lower variance, or label Unresolved
            let (chosen_pos, chosen_var, hyp_id) = if fwd_var < bwd_var * 0.5 {
                (fwd_pos, fwd_var, 0)
            } else if bwd_var < fwd_var * 0.5 {
                (bwd_pos, bwd_var, 1)
            } else {
                // Ambiguous conflict: return mean with expanded uncertainty and flag Unresolved
                let wide_var = (fwd_var + bwd_var) * 2.0 + diff * diff;
                return ReconciledFrame {
                    timestamp_ms,
                    position: ((fwd_pos + bwd_pos) * 0.5).clamp(0.0, 100.0),
                    sigma: wide_var.sqrt(),
                    observability: ObservabilityState::Unresolved,
                    dominant_hypothesis_id: 99,
                };
            };

            ReconciledFrame {
                timestamp_ms,
                position: chosen_pos.clamp(0.0, 100.0),
                sigma: chosen_var.sqrt(),
                observability: ObservabilityState::Inferred,
                dominant_hypothesis_id: hyp_id,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bayesian_bidirectional_reconciliation_agreement() {
        // Forward: pos = 80.0, var = 16.0 (sigma = 4.0)
        // Backward: pos = 84.0, var = 16.0 (sigma = 4.0)
        // Discrepancy = 4.0 <= threshold 15.0
        let reconciled = HypothesisEngine::reconcile_bidirectional(80.0, 16.0, 84.0, 16.0, 15.0, 500);

        // Fused position should be exactly 82.0
        assert!((reconciled.position - 82.0).abs() < 1e-3);
        // Fused variance = 1 / (1/16 + 1/16) = 8.0 -> sigma = sqrt(8.0) ~ 2.828
        assert!((reconciled.sigma - (8.0f32).sqrt()).abs() < 1e-3);
        assert_eq!(reconciled.observability, ObservabilityState::Observed);
    }

    #[test]
    fn test_bayesian_bidirectional_reconciliation_divergence_unresolved() {
        // Forward: pos = 20.0, var = 50.0
        // Backward: pos = 80.0, var = 50.0
        // Discrepancy = 60.0 > threshold 20.0
        let reconciled = HypothesisEngine::reconcile_bidirectional(20.0, 50.0, 80.0, 50.0, 20.0, 1000);

        // Disagreement too high: marked Unresolved
        assert_eq!(reconciled.observability, ObservabilityState::Unresolved);
        assert!(reconciled.sigma > 30.0);
    }

    #[test]
    fn test_hypothesis_tree_audio_transient_scoring() {
        let mut engine = HypothesisEngine::new();
        engine.branch_occlusion_hypotheses(70.0, 2.0, 100);

        // Initial highest likelihood is continuation
        let best_initial = engine.select_best_hypothesis().unwrap();
        assert_eq!(best_initial.label, "continuation");

        // Audio impact transient arrives
        engine.update_evidence(133, true, None);

        // Audio impact flips winning hypothesis to reversal
        let best_after = engine.select_best_hypothesis().unwrap();
        assert_eq!(best_after.label, "reversal");
    }
}
