//! Drags that stay on discrete positions, from the CA-72's `interact::drag` (R51).
//! Pointer axes and units belong to the caller. Positive distance advances a position;
//! a vertical control can pass its upward travel, a selector its rightward minus downward
//! travel. Neither a control's dimensions nor its number of positions is kit policy.

/// The position reached from `start`, counting `count` positions from zero. Each
/// `distance_a_step` of travel advances one position, multiplied by `fine_multiplier`
/// (one for ordinary travel). Half a step rounds away from the starting position.
/// Travel beyond an end stops there. A single-position control always returns zero.
///
/// Non-finite travel or rates, a non-positive step distance, a negative multiplier
/// and an empty set of positions are errors. A zero multiplier leaves the start alone.
pub fn stepped_drag(
    start: usize,
    distance: f64,
    distance_a_step: f64,
    count: usize,
    fine_multiplier: f64,
) -> Result<usize, &'static str> {
    if count == 0 {
        return Err("A stepped control needs at least one position");
    }
    if !distance.is_finite()
        || !distance_a_step.is_finite()
        || distance_a_step <= 0.0
        || !fine_multiplier.is_finite()
        || fine_multiplier < 0.0
    {
        return Err("Travel and rates must be finite, with a positive step distance");
    }
    let start = start.min(count - 1);
    if fine_multiplier == 0.0 {
        return Ok(start);
    }
    let steps = (distance / distance_a_step * fine_multiplier).round();
    // Saturating integer arithmetic also covers finite inputs whose ratio overflows.
    Ok(if steps < 0.0 {
        start.saturating_sub((-steps) as usize)
    } else {
        start.saturating_add(steps as usize).min(count - 1)
    })
}

/// A position as a share of the control's travel. Out-of-range positions are clipped;
/// one position has no travel and returns zero. An empty set is an error.
pub fn normalised(position: usize, count: usize) -> Result<f64, &'static str> {
    match count {
        0 => Err("A stepped control needs at least one position"),
        1 => Ok(0.0),
        _ => Ok(position.min(count - 1) as f64 / (count - 1) as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R51's pixel-by-pixel check, for different position counts and fine rates.
    #[test]
    fn every_drag_position_is_on_a_step() {
        for count in [1, 2, 3, 6, 9, 17] {
            for start in 0..count {
                for fine in [1.0, 0.5, 0.1] {
                    let mut previous = 0;
                    for pixel in -1200..=1200 {
                        let step = stepped_drag(start, f64::from(pixel), 24.0, count, fine)
                            .expect("valid travel");
                        assert!(step < count && step >= previous);
                        let v = normalised(step, count).expect("positions");
                        assert!((0.0..=1.0).contains(&v));
                        let position = v * count.saturating_sub(1) as f64;
                        assert!((position - position.round()).abs() < 1e-12);
                        previous = step;
                    }
                }
            }
        }
    }

    #[test]
    fn a_drag_counts_from_its_start_and_stops_at_both_ends() {
        for (distance, fine, want) in [
            (11.9, 1.0, 3),
            (12.0, 1.0, 4),
            (-12.0, 1.0, 2),
            (24.0, 1.0, 4),
            (240.0, 0.1, 4),
            (-240.0, 0.1, 2),
            (1e300, 1.0, 6),
            (-1e300, 1.0, 0),
        ] {
            assert_eq!(stepped_drag(3, distance, 24.0, 7, fine), Ok(want));
        }
        assert_eq!(stepped_drag(usize::MAX, 0.0, 1.0, 7, 1.0), Ok(6));
        assert_eq!(stepped_drag(3, f64::MAX, f64::MIN_POSITIVE, 7, 0.0), Ok(3));
        assert_eq!(stepped_drag(3, f64::MAX, f64::MIN_POSITIVE, 7, 1.0), Ok(6));
        assert_eq!(normalised(99, 4), Ok(1.0));
    }

    #[test]
    fn invalid_travel_has_no_position() {
        for (distance, spacing, rate) in [
            (f64::NAN, 1.0, 1.0),
            (f64::INFINITY, 1.0, 1.0),
            (1.0, 0.0, 1.0),
            (1.0, -1.0, 1.0),
            (1.0, f64::NAN, 1.0),
            (1.0, 1.0, f64::INFINITY),
            (1.0, 1.0, -0.1),
        ] {
            assert!(stepped_drag(0, distance, spacing, 3, rate).is_err());
        }
        assert!(stepped_drag(0, 0.0, 1.0, 0, 1.0).is_err());
        assert!(normalised(0, 0).is_err());
    }
}
