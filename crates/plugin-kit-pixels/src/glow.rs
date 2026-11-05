//! Coverage, blur and tint, from the CA-72's `glow.rs` (R-LFO). No phosphor palette,
//! core brightness, blur width or screen dimensions are chosen here.

use crate::{
    Rect,
    tiny_skia::{
        FillRule, IntSize, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Stroke,
        Transform,
    },
};

/// A coverage mask, zero to one a pixel. Shapes add their coverage, capped at one.
#[derive(Clone, Debug)]
pub struct Coverage {
    width: u32,
    height: u32,
    data: Vec<f32>,
}

impl Coverage {
    /// Non-zero dimensions representable by tiny-skia and the signed pixel coordinates.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        IntSize::from_wh(width, height)?;
        i32::try_from(width).ok()?;
        i32::try_from(height).ok()?;
        let len = (width as usize).checked_mul(height as usize)?;
        len.checked_mul(std::mem::size_of::<f32>())
            .filter(|&n| n <= isize::MAX as usize)?;
        Some(Self {
            width,
            height,
            data: vec![0.0; len],
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn data(&self) -> &[f32] {
        &self.data
    }
    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }
    pub fn clear(&mut self) {
        self.data.fill(0.0);
    }

    /// Add a same-sized tiny-skia mask. A mismatch changes nothing; invalid alpha adds nothing.
    pub fn add_mask(&mut self, mask: &Mask, alpha: f32) -> Result<(), &'static str> {
        if (mask.width(), mask.height()) != (self.width, self.height) {
            return Err("the coverage and mask have different sizes");
        }
        let alpha = unit(alpha);
        for (d, &s) in self.data.iter_mut().zip(mask.data()) {
            *d = (*d + f32::from(s) / 255.0 * alpha).min(1.0);
        }
        Ok(())
    }

    fn add_picture(&mut self, picture: &Pixmap, alpha: f32) {
        let alpha = unit(alpha);
        for (d, s) in self.data.iter_mut().zip(picture.pixels()) {
            *d = (*d + f32::from(s.alpha()) / 255.0 * alpha).min(1.0);
        }
    }

    /// Stroke in mask pixels, with round caps and joins as in the source scopes.
    pub fn stroke(&mut self, path: &Path, width: f32, alpha: f32) {
        if !width.is_finite() || width <= 0.0 || unit(alpha) == 0.0 {
            return;
        }
        let mut p = Pixmap::new(self.width, self.height).expect("the coverage's size");
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        p.stroke_path(path, &paint, &stroke, Transform::identity(), None);
        self.add_picture(&p, alpha);
    }

    /// Fill a disc in mask pixels. Non-positive or non-finite radii draw nothing.
    pub fn disc(&mut self, at: (f32, f32), radius: f32, alpha: f32) {
        if !radius.is_finite() || radius <= 0.0 || unit(alpha) == 0.0 {
            return;
        }
        let Some(path) = PathBuilder::from_circle(at.0, at.1, radius) else {
            return;
        };
        let mut p = Pixmap::new(self.width, self.height).expect("the coverage's size");
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        p.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        self.add_picture(&p, alpha);
    }

    /// Three box passes each way, approximating a Gaussian of `sigma` pixels. Outside the
    /// mask is zero, so edge light can leave it. Below 0.3 pixels is unchanged, as in the
    /// source. Invalid or unrepresentable spreads fail before changing the mask.
    pub fn blur(&mut self, sigma: f32) -> Result<(), &'static str> {
        if !sigma.is_finite() || sigma < 0.0 {
            return Err("the blur spread must be finite and non-negative");
        }
        if sigma < 0.3 {
            return Ok(());
        }
        let radius = (((12.0 * sigma * sigma / 3.0 + 1.0).sqrt() - 1.0) / 2.0)
            .round()
            .max(1.0);
        if radius >= i32::MAX as f32 {
            return Err("the blur radius is too large");
        }
        let radius = radius as usize;
        let mut tmp = vec![0.0; self.data.len()];
        for _ in 0..3 {
            box_pass(
                &self.data,
                &mut tmp,
                self.width as usize,
                self.height as usize,
                radius,
                true,
            );
            box_pass(
                &tmp,
                &mut self.data,
                self.width as usize,
                self.height as usize,
                radius,
                false,
            );
        }
        Ok(())
    }

    /// Tint the whole mask over a premultiplied pixmap. The colour and opacity are the caller's.
    pub fn tint(&self, dst: &mut Pixmap, rgb: [u8; 3], opacity: f32, at: (i32, i32)) {
        self.tint_clipped(dst, rgb, opacity, at, Rect::pixmap(dst));
    }

    /// As [`Self::tint`], only inside `clip` in the target's pixels. Blur the complete mask
    /// first, then clip its tint; blurring separate damaged pieces would change their edges.
    pub fn tint_clipped(
        &self,
        dst: &mut Pixmap,
        rgb: [u8; 3],
        opacity: f32,
        at: (i32, i32),
        clip: Rect,
    ) {
        let bounds = Rect::new(
            at.0,
            at.1,
            at.0.saturating_add(self.width as i32),
            at.1.saturating_add(self.height as i32),
        );
        let Some(r) = clip
            .intersection(Rect::pixmap(dst))
            .and_then(|r| r.intersection(bounds))
        else {
            return;
        };
        let opacity = unit(opacity);
        let w = dst.width() as usize;
        let data = dst.data_mut();
        for y in r.top..r.bottom {
            for x in r.left..r.right {
                let from = (i64::from(y) - i64::from(at.1)) as usize * self.width as usize
                    + (i64::from(x) - i64::from(at.0)) as usize;
                let a = self.data[from] * opacity;
                if a <= 0.0 {
                    continue;
                }
                let i = (y as usize * w + x as usize) * 4;
                let keep = 1.0 - a;
                for c in 0..3 {
                    data[i + c] = (f32::from(rgb[c]) * a + f32::from(data[i + c]) * keep)
                        .round()
                        .min(255.0) as u8;
                }
                data[i + 3] = (255.0 * a + f32::from(data[i + 3]) * keep)
                    .round()
                    .min(255.0) as u8;
            }
        }
    }
}

fn unit(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn box_pass(from: &[f32], to: &mut [f32], w: usize, h: usize, r: usize, across: bool) {
    let (n, lines, step, line) = if across { (w, h, 1, w) } else { (h, w, w, 1) };
    let k = 1.0 / (2 * r + 1) as f32;
    for l in 0..lines {
        let base = l * line;
        let at = |i: i64| {
            if i < 0 || i >= n as i64 {
                0.0
            } else {
                from[base + i as usize * step]
            }
        };
        // Visit the mask's pixels, never an arbitrarily wide off-mask margin.
        let mut sum: f32 = (0..=r.min(n - 1)).map(|i| from[base + i * step]).sum();
        for i in 0..n {
            to[base + i * step] = sum * k;
            sum += at(i as i64 + r as i64 + 1) - at(i as i64 - r as i64);
        }
    }
}
