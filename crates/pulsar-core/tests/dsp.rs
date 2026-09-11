use pulsar_core::*;

#[test]
fn translation_matches_vertical_motion_without_range_stretch() {
    let width = 8;
    let height = 20;
    let previous: Vec<u8> = (0..width * height)
        .map(|i| ((i * 31 + i / width * 17) % 251) as u8)
        .collect();
    let mut current = vec![0_u8; previous.len()];
    current[width..].copy_from_slice(&previous[..previous.len() - width]);
    let estimate =
        estimate_vertical_translation(&previous, &current, width as u32, height as u32, 3).unwrap();
    assert!(estimate.has_evidence);
    assert_eq!(estimate.displacement_pixels, 1.0);
    let next = integrate_vertical_motion(
        NormalizedPosition::new(0.5).unwrap(),
        estimate.displacement_pixels,
        height as u32,
    )
    .unwrap();
    assert_eq!(next.value(), 0.55);
}

#[test]
fn static_texture_is_zero_motion_but_flat_frames_are_missing_evidence() {
    let image: Vec<u8> = (0..160)
        .map(|i| ((i * 31 + i / 8 * 17) % 251) as u8)
        .collect();
    let static_estimate = estimate_vertical_translation(&image, &image, 8, 20, 3).unwrap();
    assert!(static_estimate.has_evidence);
    assert_eq!(static_estimate.displacement_pixels, 0.0);
    let flat = vec![127; 160];
    assert!(
        !estimate_vertical_translation(&flat, &flat, 8, 20, 3)
            .unwrap()
            .has_evidence
    );
}

#[test]
fn dimensions_and_resource_bounds_are_checked_before_access() {
    assert!(estimate_vertical_translation(&[], &[], 0, 0, 1).is_err());
    assert!(estimate_vertical_translation(&[0; 12], &[0; 12], 3, 5, 1).is_err());
    assert!(estimate_vertical_translation(&[0; 12], &[0; 12], 3, 4, 2).is_err());
    assert!(estimate_vertical_translation(&[0; 12], &[0; 12], 3, 4, 0).is_err());
    assert!(
        integrate_vertical_motion(NormalizedPosition::new(0.5).unwrap(), f64::NAN, 20).is_err()
    );
    assert!(integrate_vertical_motion(NormalizedPosition::new(0.5).unwrap(), 1.0, 0).is_err());
}
