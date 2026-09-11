//! Bounded proof entrypoints. These are not proof receipts. CI/developers must
//! run Kani and record the exact toolchain, revision and successful harnesses.

use crate::*;

#[kani::proof]
fn revision_increment_never_wraps() {
    let value: u64 = kani::any();
    match RevisionId::new(value).checked_next() {
        Ok(next) => {
            assert!(value < u64::MAX);
            assert_eq!(next.value(), value + 1);
        }
        Err(_) => assert_eq!(value, u64::MAX),
    }
}

#[kani::proof]
fn project_time_addition_matches_checked_integer_arithmetic() {
    let time: i64 = kani::any();
    let delta: i64 = kani::any();
    let expected = time.checked_add(delta);
    let actual = ProjectTime::from_nanos(time).checked_add(delta);
    assert_eq!(actual.ok().map(ProjectTime::as_nanos), expected);
}

#[kani::proof]
fn cancelled_job_cannot_accept_completion() {
    assert!(JobState::CancelRequested
        .transition(JobEvent::WorkerCompleted)
        .is_err());
    assert!(JobState::Cancelled
        .transition(JobEvent::WorkerCompleted)
        .is_err());
}

#[kani::proof]
#[kani::unwind(130)]
fn seek_generation_participates_in_frame_association() {
    let first: u64 = kani::any();
    let second: u64 = kani::any();
    kani::assume(first != second);
    let context = FrameContext {
        source_version: SourceVersionId::new("source").unwrap(),
        source_placement: SourcePlacementId::new("placement").unwrap(),
        frame: FrameId::new(1),
        transform: TransformId::new("transform").unwrap(),
        seek_generation: first,
        request_generation: 0,
    };
    let mut stale = context.clone();
    stale.seek_generation = second;
    assert!(!context.matches(&stale));
}
