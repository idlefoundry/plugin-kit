use plugin_kit_pixels::{
    Coverage, Rect,
    tiny_skia::{
        FillRule, LineCap, LineJoin, Mask, MaskType, Paint, PathBuilder, Pixmap, Stroke, Transform,
    },
};

#[test]
fn masks_add_and_cap_and_shapes_match_tiny_skia_coverage() {
    let mut c = Coverage::new(31, 23).expect("coverage");
    let mut p = Pixmap::new(31, 23).expect("picture");
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    let mut path = PathBuilder::new();
    path.move_to(-2.0, 1.0);
    path.line_to(14.3, 21.7);
    path.line_to(25.0, -3.0);
    let path = path.finish().expect("path");
    c.stroke(&path, 2.3, 0.6);
    p.stroke_path(
        &path,
        &paint,
        &Stroke {
            width: 2.3,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        },
        Transform::identity(),
        None,
    );
    for (&a, p) in c.data().iter().zip(p.pixels()) {
        assert_eq!(a, f32::from(p.alpha()) / 255.0 * 0.6);
    }
    c.clear();
    p.fill(plugin_kit_pixels::tiny_skia::Color::TRANSPARENT);
    c.disc((6.2, 8.7), 3.6, 0.4);
    let disc = PathBuilder::from_circle(6.2, 8.7, 3.6).expect("disc");
    p.fill_path(
        &disc,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    let mask = Mask::from_pixmap(p.as_ref(), MaskType::Alpha);
    let mut added = Coverage::new(31, 23).expect("coverage");
    added.add_mask(&mask, 0.4).expect("same size");
    assert_eq!(added.data(), c.data());
    for _ in 0..4 {
        c.add_mask(&mask, 0.4).expect("same size");
    }
    assert!(c.data().contains(&1.0));
    assert!(
        c.data()
            .iter()
            .all(|&a| a.is_finite() && (0.0..=1.0).contains(&a))
    );
    let before = c.data().to_vec();
    assert!(c.add_mask(&Mask::new(1, 1).expect("mask"), 1.0).is_err());
    c.add_mask(&mask, f32::NAN).expect("same size");
    c.stroke(&path, f32::INFINITY, 1.0);
    c.disc((1.0, 1.0), -1.0, 1.0);
    assert_eq!(c.data(), before);
    assert!(Coverage::new(0, 1).is_none());
    assert!(Coverage::new(u32::MAX, 1).is_none());
}

fn slow_pass(from: &[f32], w: usize, h: usize, radius: usize, across: bool) -> Vec<f32> {
    let mut out = vec![0.0; from.len()];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0;
            for offset in -(radius as i32)..=radius as i32 {
                let (sx, sy) = if across {
                    (x as i32 + offset, y as i32)
                } else {
                    (x as i32, y as i32 + offset)
                };
                if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                    sum += from[sy as usize * w + sx as usize];
                }
            }
            out[y * w + x] = sum / (2 * radius + 1) as f32;
        }
    }
    out
}

#[test]
fn the_blur_keeps_interior_light_and_spreads_it() {
    let mut c = Coverage::new(49, 49).expect("coverage");
    let mut mask = Mask::new(49, 49).expect("mask");
    mask.data_mut()[24 * 49 + 24] = 255;
    c.add_mask(&mask, 1.0).expect("mask");
    c.blur(3.0).expect("blur");
    let sum: f32 = c.data().iter().sum();
    assert!((sum - 1.0).abs() < 0.03, "{sum}");
    assert!(c.data()[24 * 49 + 24] < 0.1);
    assert!(c.data()[24 * 49 + 27] > 0.0);
}

#[test]
fn blur_matches_independent_box_convolution_including_edges_and_narrow_masks() {
    for (w, h) in [(1, 1), (1, 7), (7, 1), (3, 5), (23, 19)] {
        for sigma in [0.0, 0.29, 0.3, 1.0, 3.0, 9.0] {
            let mut mask = Mask::new(w, h).expect("mask");
            for (i, d) in mask.data_mut().iter_mut().enumerate() {
                *d = ((i * 79 + 13) % 256) as u8;
            }
            let mut c = Coverage::new(w, h).expect("coverage");
            c.add_mask(&mask, 1.0).expect("mask");
            let mut expected = c.data().to_vec();
            if sigma >= 0.3 {
                let r = (((4.0_f64 * f64::from(sigma).powi(2) + 1.0).sqrt() - 1.0) / 2.0)
                    .round()
                    .max(1.0) as usize;
                for _ in 0..3 {
                    expected = slow_pass(&expected, w as usize, h as usize, r, true);
                    expected = slow_pass(&expected, w as usize, h as usize, r, false);
                }
            }
            c.blur(sigma).expect("blur");
            for (&a, &b) in c.data().iter().zip(&expected) {
                assert!((a - b).abs() < 0.000_002, "{w} by {h}, {sigma}: {a} vs {b}");
            }
            let before = c.data().to_vec();
            for invalid in [-1.0, f32::NAN, f32::INFINITY, f32::MAX] {
                assert!(c.blur(invalid).is_err());
                assert_eq!(c.data(), before);
            }
        }
    }
}

#[test]
fn a_tint_drawn_in_clipped_pieces_is_the_tint_drawn_whole() {
    let mut c = Coverage::new(31, 23).expect("coverage");
    c.disc((11.2, 7.1), 4.3, 0.8);
    c.disc((29.0, 19.0), 3.0, 0.6);
    c.blur(2.4).expect("blur");
    for at in [(-13, -8), (4, 7), (37, 25), (i32::MIN, 0), (i32::MAX, 0)] {
        for opacity in [0.0, 0.2, 1.0, 3.0, -1.0, f32::NAN] {
            let initial = Pixmap::new(41, 29).expect("frame");
            let (mut whole, mut parts) = (initial.clone(), initial.clone());
            let colour = [31, 149, 213];
            c.tint(&mut whole, colour, opacity, at);
            for y in (0..29).step_by(7) {
                for x in (0..41).step_by(9) {
                    c.tint_clipped(
                        &mut parts,
                        colour,
                        opacity,
                        at,
                        Rect::new(x, y, x + 9, y + 7),
                    );
                }
            }
            assert_eq!(parts, whole, "{at:?}, {opacity}");
            assert!(
                parts.pixels().iter().all(|p| p.red() <= p.alpha()
                    && p.green() <= p.alpha()
                    && p.blue() <= p.alpha())
            );
        }
    }
}

#[test]
fn tint_is_source_over_and_leaves_every_pixel_outside_the_clip_untouched() {
    let mut mask = Mask::new(3, 2).expect("mask");
    mask.data_mut()
        .copy_from_slice(&[255, 128, 0, 32, 200, 255]);
    let mut c = Coverage::new(3, 2).expect("coverage");
    c.add_mask(&mask, 1.0).expect("mask");
    let mut p = Pixmap::new(5, 4).expect("frame");
    p.fill(plugin_kit_pixels::tiny_skia::Color::from_rgba8(
        20, 40, 60, 128,
    ));
    let initial = p.clone();
    c.tint_clipped(&mut p, [100, 150, 200], 0.5, (1, 1), Rect::new(2, 1, 4, 3));
    for y in 0..4_u32 {
        for x in 0..5_u32 {
            let old = initial.pixel(x, y).expect("inside");
            let actual = p.pixel(x, y).expect("inside");
            if (2..4).contains(&x) && (1..3).contains(&y) {
                let a = c.data()[((y - 1) * 3 + x - 1) as usize] * 0.5;
                let expected = [100, 150, 200, 255]
                    .into_iter()
                    .zip([old.red(), old.green(), old.blue(), old.alpha()])
                    .map(|(s, d)| (s as f32 * a + f32::from(d) * (1.0 - a)).round() as u8)
                    .collect::<Vec<_>>();
                assert_eq!(
                    [actual.red(), actual.green(), actual.blue(), actual.alpha()].as_slice(),
                    expected
                );
            } else {
                assert_eq!(actual, old);
            }
        }
    }
}

/// Golden f32 bits from the released CA-72 kernel: the radius lies on a rounding boundary.
#[test]
fn coverage_keeps_the_released_radius_rounding_and_box_pass_bits() {
    let mut c = Coverage::new(17, 1).unwrap();
    c.data_mut()[8] = 1.0;
    c.blur(f32::from_bits(0x409f_32af)).unwrap();
    assert_eq!(
        c.data().iter().map(|f| f.to_bits()).collect::<Vec<_>>(),
        [
            932987534, 935780441, 938883670, 940910659, 942462275, 943858726, 944634534, 945100018,
            945255180, 945100018, 944634534, 943858726, 942462275, 940910659, 938883670, 935780441,
            932987536
        ]
    );
}

#[test]
fn box_passes_keep_the_released_unclamped_round_off() {
    let mut c = Coverage::new(29, 7).unwrap();
    c.data_mut()[2 * 29 + 1] = 0.0001;
    c.blur(1.0).unwrap();
    assert_eq!(c.data()[5].to_bits(), 2838145707);
}
