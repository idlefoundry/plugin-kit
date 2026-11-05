//! General versions of the CA-72's parts/whole, sparse and window-conversion checks
//! (R44, R48, R58, R59). Fixed seeds need no random-number dependency or wall-clock deadline.

use plugin_kit_pixels::{Damage, Rect, Sparse, convert, copy, over, tiny_skia::Pixmap};

struct Random(u64);
impl Random {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn int(&mut self, low: i32, high: i32) -> i32 {
        low + (self.next() % (high - low) as u32) as i32
    }
    fn picture(&mut self, w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h).expect("picture");
        for s in p.data_mut().chunks_exact_mut(4) {
            let a = if self.next().is_multiple_of(3) {
                0
            } else {
                self.next() as u8
            };
            for c in &mut s[..3] {
                *c = (self.next() % (u32::from(a) + 1)) as u8;
            }
            s[3] = a;
        }
        p
    }
    fn rect(&mut self) -> Rect {
        Rect::new(
            self.int(-30, 90),
            self.int(-25, 70),
            self.int(-30, 90),
            self.int(-25, 70),
        )
    }
}

fn bounds(p: &Pixmap) -> Rect {
    Rect::new(0, 0, p.width() as i32, p.height() as i32)
}
fn contains(r: Rect, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// Deliberately one target pixel at a time, independent of the row/run clipping paths.
fn reference(dst: &mut Pixmap, src: &Pixmap, at: (i32, i32), clip: Rect, blend: bool) {
    let w = dst.width() as usize;
    for y in 0..dst.height() as i32 {
        for x in 0..dst.width() as i32 {
            let (sx, sy) = (
                i64::from(x) - i64::from(at.0),
                i64::from(y) - i64::from(at.1),
            );
            if !contains(clip, x, y)
                || sx < 0
                || sy < 0
                || sx >= i64::from(src.width())
                || sy >= i64::from(src.height())
            {
                continue;
            }
            let s = &src.data()[(sy as usize * src.width() as usize + sx as usize) * 4..][..4];
            let d = &mut dst.data_mut()[(y as usize * w + x as usize) * 4..][..4];
            for c in 0..4 {
                d[c] = if blend {
                    (u32::from(s[c]) + (u32::from(d[c]) * (255 - u32::from(s[3])) + 127) / 255)
                        as u8
                } else {
                    s[c]
                };
            }
        }
    }
}

fn reference_sparse(dst: &mut Pixmap, src: &Pixmap, at: (i32, i32), clip: Rect) {
    let w = dst.width() as usize;
    for y in 0..dst.height() as i32 {
        for x in 0..dst.width() as i32 {
            let (sx, sy) = (
                i64::from(x) - i64::from(at.0),
                i64::from(y) - i64::from(at.1),
            );
            if !contains(clip, x, y)
                || sx < 0
                || sy < 0
                || sx >= i64::from(src.width())
                || sy >= i64::from(src.height())
            {
                continue;
            }
            let s = &src.data()[(sy as usize * src.width() as usize + sx as usize) * 4..][..4];
            let d = &mut dst.data_mut()[(y as usize * w + x as usize) * 4..][..4];
            for c in 0..4 {
                d[c] = if s[3] > 0 {
                    (u32::from(s[c]) + (u32::from(d[c]) * (255 - u32::from(s[3]))).div_ceil(256))
                        as u8
                } else {
                    d[c]
                };
            }
        }
    }
}

#[test]
fn rectangles_merge_transitively_and_keep_separate_places_separate() {
    let mut d = Damage::new();
    for r in [
        Rect::new(0, 0, 5, 5),
        Rect::new(10, 0, 15, 5),
        Rect::new(5, 0, 10, 5),
        Rect::new(30, 20, 31, 21),
        Rect::new(1, 2, 1, 8),
        Rect::new(3, 8, 1, 1),
    ] {
        d.add(r);
    }
    assert_eq!(d.rects().len(), 4);
    d.merge(1, 8);
    assert_eq!(d.rects().len(), 2);
    assert!(d.rects().contains(&Rect::new(0, 0, 15, 5)));
    d.merge(30, 8);
    assert_eq!(d.rects(), &[Rect::new(0, 0, 31, 21)]);
    d.clear();
    for x in 0..9 {
        d.add(Rect::new(x * 20, 0, x * 20 + 2, 2));
    }
    d.merge(0, 8);
    assert_eq!(d.rects(), &[Rect::new(0, 0, 162, 2)]);
    d.add(Rect::new(i32::MIN, i32::MIN, -1, -1));
    d.add(Rect::new(i32::MAX - 1, i32::MAX - 1, i32::MAX, i32::MAX));
    d.merge(u32::MAX, 0);
    assert_eq!(
        d.rects(),
        &[Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX)]
    );
}

#[test]
fn a_change_above_a_component_draws_none_of_it() {
    let component = Rect::new(0, 50, 100, 90);
    let mut d = Damage::new();
    d.add_clipped(Rect::new(80, 2, 100, 40), component);
    d.add_clipped(Rect::new(40, 65, 60, 75), component);
    assert_eq!(d.rects(), &[Rect::new(40, 65, 60, 75)]);
    assert!(d.clipped(Rect::new(0, 0, 100, 50)).is_empty());
    d.add_clipped(Rect::new(-5, 88, 120, 100), component);
    assert!(d.rects().contains(&Rect::new(0, 88, 100, 90)));
}

#[test]
fn sparse_dense_and_pixel_composition_agree_over_random_clipping() {
    let mut random = Random(0x9a74_4448);
    for i in 0..500 {
        let src = random.picture(1 + (i % 37), 1 + (i % 23));
        let sparse = Sparse::from_pixmap(&src);
        let initial = random.picture(53, 39);
        let (mut dense, mut runs, mut slow, mut copied) = (
            initial.clone(),
            initial.clone(),
            initial.clone(),
            initial.clone(),
        );
        let at = if i < 4 {
            [(i32::MIN, 0), (i32::MAX, 0), (0, i32::MIN), (0, i32::MAX)][i as usize]
        } else {
            (random.int(-45, 70), random.int(-35, 55))
        };
        let clip = if i % 3 == 0 {
            Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX)
        } else {
            random.rect()
        };
        over(&mut dense, &src, at, clip);
        sparse.over(&mut runs, at, clip);
        reference(&mut slow, &src, at, clip, true);
        let mut sparse_reference = initial.clone();
        reference_sparse(&mut sparse_reference, &src, at, clip);
        assert_eq!(runs, sparse_reference, "sparse {i}, {at:?}, {clip:?}");
        assert_eq!(dense, slow, "dense {i}, {at:?}, {clip:?}");
        copy(&mut copied, &src, at, clip);
        slow.clone_from(&initial);
        reference(&mut slow, &src, at, clip, false);
        assert_eq!(copied, slow, "copy {i}, {at:?}, {clip:?}");
    }
}

#[derive(Clone)]
struct Layer {
    image: Pixmap,
    at: (i32, i32),
    clip: Rect,
}
impl Layer {
    fn rect(&self) -> Rect {
        Rect::new(
            self.at.0,
            self.at.1,
            self.at.0 + self.image.width() as i32,
            self.at.1 + self.image.height() as i32,
        )
    }
}

fn compose(
    frame: &mut Pixmap,
    background: &Pixmap,
    layers: &[Layer],
    damage: &Damage,
    sparse: bool,
) {
    for &r in damage.rects() {
        copy(frame, background, (0, 0), r);
        for layer in layers {
            if let Some(clip) = r.intersection(layer.clip) {
                if sparse {
                    Sparse::from_pixmap(&layer.image).over(frame, layer.at, clip);
                } else {
                    reference_sparse(frame, &layer.image, layer.at, clip);
                }
            }
        }
    }
}

#[test]
fn a_frame_drawn_in_parts_is_the_frame_drawn_whole() {
    for seed in [44, 48, 58, 59, 60, 0x1278_acdf] {
        let mut random = Random(seed);
        let background = random.picture(71, 53);
        let all = bounds(&background);
        let mut layers: Vec<_> = (0..7)
            .map(|i| Layer {
                image: random.picture(13 + i, 11 + i),
                at: (random.int(-10, 65), random.int(-10, 45)),
                clip: all,
            })
            .collect();
        let mut parts = background.clone();
        compose(&mut parts, &background, &layers, &Damage::full(all), true);
        let mut window = vec![0; parts.pixels().len()];
        convert(&parts, &mut window, &Damage::full(all)).expect("convert");
        for frame in 0..100 {
            let mut damage = Damage::new();
            for _ in 0..1 + frame % 3 {
                let i = random.next() as usize % layers.len();
                damage.add_clipped(layers[i].rect(), layers[i].clip);
                let l = &mut layers[i];
                l.at = (random.int(-25, 80), random.int(-20, 60));
                let width = 1 + random.next() % 27;
                l.image = random.picture(width, 1 + frame % 21);
                l.clip = if frame % 4 == 0 { random.rect() } else { all };
                damage.add_clipped(l.rect(), l.clip);
            }
            let mut damage = damage.clipped(all);
            damage.merge(frame % 4, 5);
            compose(&mut parts, &background, &layers, &damage, true);
            let mut whole = background.clone();
            compose(&mut whole, &background, &layers, &Damage::full(all), false);
            assert_eq!(parts, whole, "seed {seed}, frame {frame}, {damage:?}");
            let age = [0, 1, 1, 2, 1][frame as usize % 5];
            let complete = frame % 13 != 0;
            let shown = Damage::for_buffer(all, Some(&damage), age, complete);
            if age != 1 || !complete {
                window.fill(0xdead_beef);
            }
            convert(&parts, &mut window, &shown).expect("convert");
            let mut expected = vec![0; window.len()];
            convert(&whole, &mut expected, &Damage::full(all)).expect("convert");
            assert_eq!(window, expected, "window seed {seed}, frame {frame}");
        }
    }
}

#[test]
fn conversion_only_touches_clipped_damage_and_keeps_rgb_bytes() {
    let mut random = Random(0x6038);
    let image = random.picture(41, 29);
    for _ in 0..100 {
        let mut damage = Damage::new();
        for _ in 0..9 {
            damage.add(random.rect());
        }
        let mut target = vec![0x1234_5678; image.pixels().len() + 17];
        convert(&image, &mut target, &damage).expect("convert");
        for (i, (s, &d)) in image.data().chunks_exact(4).zip(&target).enumerate() {
            let (x, y) = ((i % 41) as i32, (i / 41) as i32);
            let expected = if damage.rects().iter().any(|&r| contains(r, x, y)) {
                u32::from_be_bytes([255, s[0], s[1], s[2]])
            } else {
                0x1234_5678
            };
            assert_eq!(d, expected, "{x}, {y}");
        }
        assert!(
            target[image.pixels().len()..]
                .iter()
                .all(|&v| v == 0x1234_5678)
        );
    }
    let mut short = vec![7; image.pixels().len() - 1];
    assert!(convert(&image, &mut short, &Damage::full(bounds(&image))).is_err());
    assert!(short.iter().all(|&v| v == 7));
}

#[test]
fn partial_presentation_requires_known_damage_a_complete_frame_and_age_one() {
    let all = Rect::new(0, 0, 41, 29);
    let changes = Damage::full(Rect::new(-8, 3, 9, 40));
    assert_eq!(
        Damage::for_buffer(all, Some(&changes), 1, true).rects(),
        &[Rect::new(0, 3, 9, 29)]
    );
    for age in [0, 2, 255] {
        assert_eq!(
            Damage::for_buffer(all, Some(&changes), age, true),
            Damage::full(all)
        );
    }
    assert_eq!(
        Damage::for_buffer(all, Some(&changes), 1, false),
        Damage::full(all)
    );
    assert_eq!(Damage::for_buffer(all, None, 1, true), Damage::full(all));
    assert!(Damage::for_buffer(all, Some(&Damage::new()), 1, true).is_empty());
}

#[test]
fn damage_defers_merging_and_preserves_the_released_order_and_strict_gap() {
    let rects = [
        Rect::new(0, 0, 5, 5),
        Rect::new(40, 0, 45, 5),
        Rect::new(6, 0, 11, 5),
        Rect::new(80, 0, 85, 5),
    ];
    let mut damage = Damage::new();
    for r in rects {
        damage.add(r);
    }
    assert_eq!(damage.rects(), &rects);
    damage.merge(1, 8);
    assert_eq!(damage.rects(), &rects);
    damage.merge(2, 8);
    assert_eq!(
        damage.rects(),
        &[Rect::new(0, 0, 11, 5), rects[1], rects[3]]
    );
    let mut eager = Damage::with_eager_merging(1);
    for r in rects {
        eager.add(r);
    }
    assert_eq!(eager.rects(), &[rects[1], Rect::new(0, 0, 11, 5), rects[3]]);
}

#[test]
fn sparse_uses_the_released_pipeline_division() {
    let mut source = Pixmap::new(1, 1).unwrap();
    source.data_mut().copy_from_slice(&[1, 2, 0, 2]);
    let mut target = Pixmap::new(1, 1).unwrap();
    target.data_mut().copy_from_slice(&[64, 64, 64, 255]);
    Sparse::new(source).over(&mut target, (0, 0), Rect::new(0, 0, 1, 1));
    assert_eq!(target.data(), &[65, 66, 64, 255]);
}
