//! The materials of the plug-ins' worn skins (decisions.md K5): pictures placed and resampled at
//! a scale, the displays' glass, a lit button's light on the surface round it and its
//! translucent cap lit from inside, dots filled as one shape, and the displays' lettering in
//! dots. From the CA-74's worn skin (its R36 to R38); each plug-in brings its own pictures and
//! colours (the CA-74's red: [`RED`], [`RED_LIGHT`]). Everything is drawn into a tiny-skia
//! pixmap, in a plug-in's units at a scale (pixels a unit).

use std::fmt::{self, Display, Write as _};

use resvg::tiny_skia::{
    ColorU8, FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, PixmapPaint, SpreadMode,
    Transform,
};

/// A number as SVG wants it: at most three decimals, no trailing zeros (the plug-ins' own).
#[derive(Clone, Copy, Debug)]
pub struct N(pub f64);

impl Display for N {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = (self.0 * 1000.0).round() / 1000.0;
        if v == 0.0 {
            f.write_str("0")
        } else {
            write!(f, "{v}")
        }
    }
}

/// A rounded rectangle's path, in pixels.
pub fn rounded_rect(x: f64, y: f64, w: f64, h: f64, r: f64) -> Option<resvg::tiny_skia::Path> {
    let (x, y, w, h) = (x as f32, y as f32, w as f32, h as f32);
    let r = (r as f32).min(w / 2.0).min(h / 2.0);
    // A quarter circle's control points, as a share of its radius.
    let k = 0.552_284_8 * r;
    let mut p = PathBuilder::new();
    p.move_to(x + r, y);
    p.line_to(x + w - r, y);
    p.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    p.line_to(x + w, y + h - r);
    p.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    p.line_to(x + r, y + h);
    p.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    p.line_to(x, y + r);
    p.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    p.close();
    p.finish()
}

/// `src` resized to `w` by `h`: halved in steps while it is more than twice as large (each
/// step the mean of four pixels, so a large picture does not alias), then drawn to size.
pub fn resample(src: &Pixmap, w: u32, h: u32) -> Pixmap {
    let mut cur = src.clone();
    while cur.width() >= 2 * w && cur.height() >= 2 * h && cur.width() > 1 && cur.height() > 1 {
        let (hw, hh) = (cur.width().div_ceil(2), cur.height().div_ceil(2));
        let mut half = Pixmap::new(hw, hh).expect("a picture of at least a pixel");
        half.draw_pixmap(
            0,
            0,
            cur.as_ref(),
            &PixmapPaint {
                quality: FilterQuality::Bilinear,
                ..PixmapPaint::default()
            },
            Transform::from_scale(
                hw as f32 / cur.width() as f32,
                hh as f32 / cur.height() as f32,
            ),
            None,
        );
        cur = half;
    }
    let mut out = Pixmap::new(w, h).expect("a picture of at least a pixel");
    out.draw_pixmap(
        0,
        0,
        cur.as_ref(),
        &PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..PixmapPaint::default()
        },
        Transform::from_scale(
            w as f32 / cur.width() as f32,
            h as f32 / cur.height() as f32,
        ),
        None,
    );
    out
}

/// A lit button's light on the surface round it ([`Glow`]): the colour at the gap under the
/// cap's edge, and the colour it throws wider. Linear light, 0 to 1 a channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Underlight {
    pub rim: [f32; 3],
    pub light: [f32; 3],
}

/// The CA-74's: red, its readouts' (its owner, 2026-10-08: "I think i might like the red better
/// actually"; tried, an Edison bulb's orange, `#ff8519` and `#ffbd6e`, and its displays' teal,
/// `#00d2a8` and `#24ecc4`, which "doesn't look natural").
pub const RED_LIGHT: Underlight = Underlight {
    rim: [1.0, 82.0 / 255.0, 50.0 / 255.0],   // #ff5232
    light: [1.0, 36.0 / 255.0, 18.0 / 255.0], // #ff2412
};

/// A picture already at its size in pixels, centred at (`x`, `y`) (the strip's units).
pub fn place(frame: &mut Pixmap, p: &Pixmap, scale: f64, (x, y): (f64, f64), paint: &PixmapPaint) {
    let (left, top) = (
        x * scale - f64::from(p.width()) / 2.0,
        y * scale - f64::from(p.height()) / 2.0,
    );
    frame.draw_pixmap(
        0,
        0,
        p.as_ref(),
        paint,
        Transform::from_translate(left as f32, top as f32),
        None,
    );
}

/// A picture resampled to `w` by `h` (the strip's units) and centred at (`x`, `y`).
pub fn sprite(frame: &mut Pixmap, p: &Pixmap, scale: f64, at: (f64, f64), (w, h): (f64, f64)) {
    let px = |v: f64| (v * scale).round().max(1.0) as u32;
    let p = resample(p, px(w), px(h));
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    place(frame, &p, scale, at, &paint);
}

/// The display's glass filling a rounded window (the strip's units), covering it about its
/// middle as the mock-up's did.
pub fn glass(
    frame: &mut Pixmap,
    p: &Pixmap,
    scale: f64,
    (x, y, w, h): (f64, f64, f64, f64),
    r: f64,
) {
    let Some(path) = rounded_rect(x * scale, y * scale, w * scale, h * scale, r * scale) else {
        return;
    };
    let (pw, ph) = (f64::from(p.width()), f64::from(p.height()));
    let k = (w * scale / pw).max(h * scale / ph);
    let (ox, oy) = (
        x * scale + (w * scale - pw * k) / 2.0,
        y * scale + (h * scale - ph * k) / 2.0,
    );
    let paint = Paint {
        shader: Pattern::new(
            p.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bicubic,
            1.0,
            Transform::from_scale(k as f32, k as f32).post_translate(ox as f32, oy as f32),
        ),
        anti_alias: true,
        ..Paint::default()
    };
    frame.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// A five-pointed star's path at (`x`, `y`), `r` its points' radius.
pub fn star_path(x: f64, y: f64, r: f64) -> String {
    let mut d = String::new();
    for i in 0..10 {
        let a = std::f64::consts::PI * f64::from(i) / 5.0 - std::f64::consts::FRAC_PI_2;
        let rr = if i % 2 == 0 { r } else { r * 0.42 };
        d.push_str(&format!(
            "{}{} {} ",
            if i == 0 { "M" } else { "L" },
            N(x + rr * a.cos()),
            N(y + rr * a.sin())
        ));
    }
    d.push('Z');
    d
}

/// A cap `k` times as wide as it is tall, made from the square one: its left and right
/// halves, the middle column drawn out between them.
pub fn widened(p: &Pixmap, k: f64) -> Pixmap {
    let (w, h) = (p.width(), p.height());
    let nw = (f64::from(w) * k).round() as u32;
    let mut out = Pixmap::new(nw, h).expect("a cap");
    let half = w / 2;
    for y in 0..h {
        for x in 0..nw {
            let sx = if x < half {
                x
            } else if x >= nw - half {
                x - (nw - w)
            } else {
                half
            };
            let i = ((y * w + sx) * 4) as usize;
            let o = ((y * nw + x) * 4) as usize;
            out.data_mut()[o..o + 4].copy_from_slice(&p.data()[i..i + 4]);
        }
    }
    out
}

/// Dots (a path of circles) filled in a colour.
pub fn fill(frame: &mut Pixmap, pb: PathBuilder, (r, g, b): (u8, u8, u8), a: f64) {
    if let Some(path) = pb.finish() {
        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        paint.set_color_rgba8(r, g, b, (a * 255.0).round().clamp(0.0, 255.0) as u8);
        frame.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// A disc: its middle and how far across from it it reaches, in pixels.
pub type Disc = (f32, f32, f32);

/// Discs at (`x`, `y`), `r` across from their middles (in pixels), filled over `frame` as one
/// shape (where they overlap, covered once), in `colour` at `alpha`: as a path of circles is
/// filled, its rims smoothed over a pixel, but in a fraction of the time (thousands of a
/// display's dots, as a path, took most of a frame). `cover` is the frame's size, nothing in
/// it, and is left so.
pub fn discs(
    frame: &mut Pixmap,
    cover: &mut [u8],
    at: &[Disc],
    (r, g, b): (u8, u8, u8),
    alpha: f32,
) {
    let (w, h) = (frame.width() as i32, frame.height() as i32);
    if cover.len() != (w * h) as usize {
        return;
    }
    let bounds = |&(x, y, rad): &Disc| {
        (
            ((x - rad - 1.0).floor() as i32).max(0),
            ((y - rad - 1.0).floor() as i32).max(0),
            ((x + rad + 1.0).ceil() as i32).min(w),
            ((y + rad + 1.0).ceil() as i32).min(h),
        )
    };
    for d in at {
        let &(x, y, rad) = d;
        let (x0, y0, x1, y1) = bounds(d);
        let edge = |px: i32, py: i32| {
            let (dx, dy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
            (rad - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
        };
        // (A small disc's rim, smoothed over a pixel, would cover more than the disc: it covers
        // its area.)
        let k = if rad < 2.0 {
            let sum: f32 = (y0..y1)
                .flat_map(|py| (x0..x1).map(move |px| (px, py)))
                .map(|(px, py)| edge(px, py))
                .sum();
            if sum > 0.0 {
                (std::f32::consts::PI * rad * rad / sum).min(1.0)
            } else {
                0.0
            }
        } else {
            1.0
        };
        for py in y0..y1 {
            for px in x0..x1 {
                let c = (edge(px, py) * k * 255.0).round() as u8;
                let i = (py * w + px) as usize;
                cover[i] = cover[i].max(c);
            }
        }
    }
    let data = frame.data_mut();
    for d in at {
        let (x0, y0, x1, y1) = bounds(d);
        for py in y0..y1 {
            for px in x0..x1 {
                let i = (py * w + px) as usize;
                let c = std::mem::take(&mut cover[i]);
                if c == 0 {
                    continue;
                }
                let a = alpha * f32::from(c) / 255.0;
                let p = &mut data[4 * i..4 * i + 4];
                for (j, v) in [r, g, b, 255].into_iter().enumerate() {
                    p[j] = (f32::from(v) * a + f32::from(p[j]) * (1.0 - a)).round() as u8;
                }
            }
        }
    }
}

/// A lit cap's light on the surface round it, made once for where the cap is at a scale (only
/// the patch round it): brightest right at the gap under the cap's edge, falling off sharply
/// (the owner: "keep the peak brightness very close to the edges with a sharp dropoff"). The cap
/// is drawn over it after (it stops the light over its top).
#[derive(Debug)]
pub struct Glow {
    /// The patch's top left corner in the frame, and its size, in pixels.
    left: i32,
    top: i32,
    w: usize,
    h: usize,
    /// The light falling on each of the patch's pixels: its colour and strength (0 to
    /// `u16::MAX`).
    light: Vec<[u16; 3]>,
}

impl Glow {
    /// The light of the lamp in the cap at (`x`, `y`), `cw` by `ch` (the strip's units), at
    /// `scale`: bands of light round the cap's edge, each a rounded rectangle blurred, each over
    /// the last (as an SVG of them, blurred by its filters, draws them; drawn here, it is made in
    /// a fraction of the time).
    pub fn new(
        scale: f64,
        (x, y): (f64, f64),
        (cw, ch): (f64, f64),
        colours: Underlight,
    ) -> Option<Glow> {
        /// How far round the cap its light reaches: the widest band's edge and four of its
        /// blur's widths.
        const REACH: f64 = 6.0 + 4.0 * 8.0;
        let left = ((x - cw / 2.0 - REACH) * scale).floor();
        let top = ((y - ch / 2.0 - REACH) * scale).floor();
        let right = ((x + cw / 2.0 + REACH) * scale).ceil();
        let bottom = ((y + ch / 2.0 + REACH) * scale).ceil();
        let (w, h) = ((right - left) as usize, (bottom - top) as usize);
        let mut mask = Pixmap::new(w as u32, h as u32)?;
        let mut alpha = vec![0.0; w * h];
        let mut across = vec![0.0; w * h];
        let mut light = vec![[0.0f32; 3]; w * h];
        // (The gap's, hot and close; the light it throws, wider; its tail, faint and wide: how
        // much each grows the cap, its blur, its colour and its strength.)
        for (grow, blur, colour, strength) in [
            (2.0, 1.4, colours.rim, 0.9),
            (4.5, 3.5, colours.light, 0.7),
            (12.0, 8.0, colours.light, 0.15),
        ] {
            let (bw, bh) = (cw + grow, ch + grow);
            let Some(path) = rounded_rect(
                (x - bw / 2.0) * scale - left,
                (y - bh / 2.0) * scale - top,
                bw * scale,
                bh * scale,
                (11.0 + grow / 2.0) * scale,
            ) else {
                continue;
            };
            mask.fill(resvg::tiny_skia::Color::TRANSPARENT);
            mask.fill_path(
                &path,
                &Paint::default(),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            for (a, m) in alpha.iter_mut().zip(mask.pixels()) {
                *a = f32::from(m.alpha()) / 255.0;
            }
            let sigma = (blur * scale) as f32;
            blurred(&alpha, &mut across, w, h, sigma);
            blurred(&across, &mut alpha, h, w, sigma);
            for (l, &a) in light.iter_mut().zip(&alpha) {
                let a = a * strength;
                for c in 0..3 {
                    l[c] = l[c] * (1.0 - a) + colour[c] * a;
                }
            }
        }
        let full = f32::from(u16::MAX);
        Some(Glow {
            left: left as i32,
            top: top as i32,
            w,
            h,
            light: light
                .into_iter()
                .map(|l| l.map(|v| (v.clamp(0.0, 1.0) * full).round() as u16))
                .collect(),
        })
    }

    /// Its light added to `frame` as light is to a surface, in linear light: the surface's own
    /// colour lit by it (a cream face takes its colour and brightens, a black one hardly), and a
    /// little of it seen in the air over any surface (so it shows on black too), rolled off
    /// softly as it nears white rather than clipped.
    pub fn light(&self, frame: &mut Pixmap) {
        /// How strongly the light lights the surface, and how much of it is seen over any.
        const LIT: f32 = 2.8;
        const HAZE: f32 = 0.6;
        let (fw, fh) = (frame.width() as i32, frame.height() as i32);
        let (lw, lh) = (self.w as i32, self.h as i32);
        let (x0, x1) = (self.left.max(0), (self.left + lw).min(fw));
        let (y0, y1) = (self.top.max(0), (self.top + lh).min(fh));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let linear = &tables().linear;
        let unit = 1.0 / f32::from(u16::MAX);
        let to = frame.data_mut();
        for y in y0..y1 {
            let row = (4 * (y * fw + x0)) as usize..(4 * (y * fw + x1)) as usize;
            let from = ((y - self.top) * lw + x0 - self.left) as usize;
            let from = &self.light[from..from + (x1 - x0) as usize];
            for (px, l) in to[row].chunks_exact_mut(4).zip(from) {
                if *l == [0; 3] {
                    continue;
                }
                for c in 0..3 {
                    let (b, li) = (linear[usize::from(px[c])], f32::from(l[c]) * unit);
                    px[c] = encoded(b * (1.0 + LIT * li) + HAZE * li);
                }
            }
        }
    }
}

/// `from`, `count` lines of `len`, each line blurred as by a Gaussian of `sigma` pixels, into
/// `to` turned (its lines `from`'s columns: blurred along rows, then along the rows of that, a
/// picture is blurred both ways and turned back). Narrow, by its kernel; wide, by three boxes,
/// near enough the same and quicker. Beyond the line, nothing.
fn blurred(from: &[f32], to: &mut [f32], len: usize, count: usize, sigma: f32) {
    let turned = |to: &mut [f32], line: usize, out: &[f32]| {
        for (k, &v) in out.iter().enumerate() {
            to[k * count + line] = v;
        }
    };
    let mut out = vec![0.0; len];
    if sigma < 0.3 {
        for line in 0..count {
            turned(to, line, &from[line * len..(line + 1) * len]);
        }
        return;
    }
    if sigma < 3.0 {
        let r = (3.0 * sigma).ceil() as usize;
        let mut kernel: Vec<f32> = (0..=2 * r)
            .map(|i| (-((i as f32 - r as f32).powi(2)) / (2.0 * sigma * sigma)).exp())
            .collect();
        let sum: f32 = kernel.iter().sum();
        kernel.iter_mut().for_each(|k| *k /= sum);
        for line in 0..count {
            let src = &from[line * len..(line + 1) * len];
            for (k, o) in out.iter_mut().enumerate() {
                let (lo, hi) = (k.saturating_sub(r), (k + r).min(len - 1));
                *o = (lo..=hi).map(|t| src[t] * kernel[t + r - k]).sum();
            }
            turned(to, line, &out);
        }
        return;
    }
    // Three boxes whose blur is the Gaussian's, near enough: of two odd widths, as many of
    // each as come nearest its spread.
    let ideal = (4.0 * sigma * sigma + 1.0).sqrt();
    let mut lower = ideal.floor() as usize;
    if lower.is_multiple_of(2) {
        lower -= 1;
    }
    let l = lower as f32;
    let lowers = ((12.0 * sigma * sigma - 3.0 * l * l - 12.0 * l - 9.0) / (-4.0 * l - 4.0))
        .round()
        .clamp(0.0, 3.0) as usize;
    let mut a = vec![0.0; len];
    for line in 0..count {
        a.copy_from_slice(&from[line * len..(line + 1) * len]);
        for pass in 0..3 {
            let r = (if pass < lowers { lower } else { lower + 2 }) / 2;
            let k = 1.0 / (2 * r + 1) as f32;
            let mut sum: f32 = a[..r.min(len)].iter().sum();
            for (i, o) in out.iter_mut().enumerate() {
                if i + r < len {
                    sum += a[i + r];
                }
                *o = sum * k;
                if i >= r {
                    sum -= a[i - r];
                }
            }
            a.copy_from_slice(&out);
        }
        turned(to, line, &a);
    }
}

/// The levels' light, and light's levels (made once).
struct Tables {
    /// Each sRGB level's light (0 to 1, linear).
    linear: [f32; 256],
    /// Light's level, by its square root (finer where the levels are closer, near black), up to
    /// `BRIGHTEST`.
    levels: Vec<u8>,
}

/// The most light a level is looked up for: a white surface lit as brightly as a lamp lights it.
const BRIGHTEST: f32 = 4.5;
/// The levels' table's steps for each unit of light's square root.
const STEPS: f32 = 4096.0;

fn tables() -> &'static Tables {
    static TABLES: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| Tables {
        linear: std::array::from_fn(|v| linear(v as u8)),
        levels: (0..=(BRIGHTEST.sqrt() * STEPS).ceil() as u32)
            .map(|i| level((i as f32 / STEPS).powi(2)))
            .collect(),
    })
}

/// An sRGB level's light (0 to 1, linear).
fn linear(v: u8) -> f32 {
    let v = f32::from(v) / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Light (linear) as an sRGB level, from the table.
fn encoded(v: f32) -> u8 {
    let levels = &tables().levels;
    let i = (v.max(0.0).sqrt() * STEPS + 0.5) as usize;
    levels[i.min(levels.len() - 1)]
}

/// Light (linear) as an sRGB level, rolled off softly towards white above `KNEE` rather than
/// clipped.
fn level(v: f32) -> u8 {
    const KNEE: f32 = 0.8;
    let v = if v <= KNEE {
        v
    } else {
        KNEE + (1.0 - KNEE) * (1.0 - (-(v - KNEE) / (1.0 - KNEE)).exp())
    };
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The caps of the buttons that light, unlit and lit ([`lighting`]).
#[derive(Debug)]
pub struct Lighting {
    pub unlit: Pixmap,
    pub lit: Pixmap,
}

/// The colour of a translucent cap ([`lighting`]): lit, of its glow `g` (the light from inside,
/// about 0 to 1.5, brightest over its middle), and unlit, of its shade `k` (the opaque cap's
/// shading carried over, about 0.3 to 2.6); linear light, a channel each.
#[derive(Clone, Copy, Debug)]
pub struct Tint {
    pub lit: fn(f32) -> [f32; 3],
    pub unlit: fn(f32) -> [f32; 3],
}

/// The CA-74's red (its owner, 2026-10-08: "those old school semi-translucent buttons from the
/// 70's that would glow"): unlit, a deep red; lit, hot towards orange over the middle, a deeper
/// red towards its edge. (Amber was tried there: lit `[1.25, 0.55 g^1.6, 0.08 g^3]` of the
/// glow `g`, unlit `[0.26, 0.062, 0.003]` of the shade.)
pub const RED: Tint = Tint {
    lit: |glow| {
        let (g2, g3) = (glow * glow, glow * glow * glow);
        [1.25 * glow, 0.34 * g3 * glow.sqrt(), 0.14 * g2 * g2]
    },
    unlit: |k| [0.19 * k, 0.017 * k, 0.013 * k],
};

/// A lighting button's cap, unlit and lit, `k` times as wide as it is tall ([`widened`]): an
/// opaque cap's picture (`button`, square), its shape, dish, rim and grain, made translucent
/// plastic of `tint`. Unlit, shaded as the opaque cap is; lit, glowing from a lamp inside:
/// brightest over its middle, and a little brighter again round its walls, where the plastic
/// carries the light to its edge. Made once a plug-in (its strip and drop-down share them).
pub fn lighting(button: &Pixmap, k: f64, tint: Tint) -> Lighting {
    let (unlit, lit) = translucent(&widened(button, k), tint);
    Lighting { unlit, lit }
}

fn translucent(button: &Pixmap, tint: Tint) -> (Pixmap, Pixmap) {
    /// The charcoal's mean level over the cap's top: a pixel's level over it is its shading.
    const MEAN: f32 = 0.29;
    let (w, h) = (button.width(), button.height());
    let (half, long) = (h as f32 / 2.0, (w as f32 - h as f32).max(0.0) / 2.0);
    let (mut unlit, mut lit) = (button.clone(), button.clone());
    for (i, (off, on)) in unlit
        .pixels_mut()
        .iter_mut()
        .zip(lit.pixels_mut().iter_mut())
        .enumerate()
    {
        let c = off.demultiply();
        if c.alpha() == 0 {
            continue;
        }
        let level = (0.2126 * f32::from(c.red())
            + 0.7152 * f32::from(c.green())
            + 0.0722 * f32::from(c.blue()))
            / 255.0;
        let shade = (level / MEAN).clamp(0.3, 2.0);
        // How far out from the middle, as the cap's rounded square goes (a wider cap's middle
        // drawn out across): 0 there, 1 at its edge.
        let (x, y) = ((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5);
        let u = ((x - w as f32 / 2.0).abs() - long).max(0.0) / half;
        let v = (y - half).abs() / half;
        let d = (u * u * u * u + v * v * v * v).sqrt().sqrt().min(1.0);
        let paint = |[r, g, b]: [f32; 3]| {
            ColorU8::from_rgba(encoded(r), encoded(g), encoded(b), c.alpha()).premultiply()
        };
        // Lit: its middle, its walls; unlit, darker over the middle, where one sees into it,
        // than round its walls.
        let middle = 1.0 - 0.5 * smooth(d / 0.95);
        let walls = 0.45 * (-(1.0 - d) / 0.05).exp();
        let glow = 0.85 * (middle + walls) * (0.8 + 0.2 * shade);
        *on = paint((tint.lit)(glow));
        let k = shade * shade.powf(0.4) * (0.75 + 0.35 * d);
        *off = paint((tint.unlit)(k));
    }
    (unlit, lit)
}

/// 0 below 0, 1 above 1, and smoothly between.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Over a lit cap drawn again, as SVG (in the plug-in's units): the panel's lamp on it as on
/// every cap (its shade fainter, the cap glowing from inside). Nothing for no caps.
pub fn lit_caps(at: &[(f64, f64)], (w, h): (f64, f64)) -> String {
    let mut s = String::new();
    if at.is_empty() {
        return s;
    }
    let _ = write!(
        s,
        "<defs><linearGradient id='litcap' x1='0' y1='0' x2='1' y2='1'><stop offset='0' stop-color='#fff' stop-opacity='0.16'/><stop offset='0.42' stop-color='#fff' stop-opacity='0'/><stop offset='0.58' stop-color='#000' stop-opacity='0'/><stop offset='1' stop-color='#000' stop-opacity='0.08'/></linearGradient></defs>"
    );
    for &(x, y) in at {
        let _ = write!(
            s,
            "<rect x='{}' y='{}' width='{}' height='{}' rx='10' fill='url(#litcap)'/>",
            N(x - w / 2.0 + 3.0),
            N(y - h / 2.0 + 3.0),
            N(w - 6.0),
            N(h - 6.0)
        );
    }
    s
}

/// The display's lettering: five dots by seven a character, two rows below for a descender,
/// after the dot-matrix character sets of the late 1970s' displays (drawn here; no typeface is
/// built in for it).
pub mod font {
    /// The printable ASCII characters' rows, from the space; each row five dots, the leftmost
    /// the highest bit.
    #[rustfmt::skip]
    const ASCII: [[u8; 7]; 95] = [
        [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // ' '
        [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04], // !
        [0x0A, 0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00], // "
        [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A], // #
        [0x04, 0x0F, 0x14, 0x0E, 0x05, 0x1E, 0x04], // $
        [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03], // %
        [0x0C, 0x12, 0x14, 0x08, 0x15, 0x12, 0x0D], // &
        [0x04, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00], // '
        [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02], // (
        [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08], // )
        [0x00, 0x04, 0x15, 0x0E, 0x15, 0x04, 0x00], // *
        [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00], // +
        [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08], // ,
        [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00], // -
        [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C], // .
        [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x00], // /
        [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // 0
        [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], // 1
        [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F], // 2
        [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E], // 3
        [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], // 4
        [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], // 5
        [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E], // 6
        [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
        [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], // 8
        [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], // 9
        [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00], // :
        [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x04, 0x08], // ;
        [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02], // <
        [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00], // =
        [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08], // >
        [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04], // ?
        [0x0E, 0x11, 0x01, 0x0D, 0x15, 0x15, 0x0E], // @
        [0x0E, 0x11, 0x11, 0x11, 0x1F, 0x11, 0x11], // A
        [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E], // B
        [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E], // C
        [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C], // D
        [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F], // E
        [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10], // F
        [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F], // G
        [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11], // H
        [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E], // I
        [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C], // J
        [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11], // K
        [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F], // L
        [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11], // M
        [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11], // N
        [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E], // O
        [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10], // P
        [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D], // Q
        [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11], // R
        [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E], // S
        [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04], // T
        [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E], // U
        [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04], // V
        [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A], // W
        [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11], // X
        [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04], // Y
        [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F], // Z
        [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E], // [
        [0x00, 0x10, 0x08, 0x04, 0x02, 0x01, 0x00], // \
        [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E], // ]
        [0x04, 0x0A, 0x11, 0x00, 0x00, 0x00, 0x00], // ^
        [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F], // _
        [0x08, 0x04, 0x02, 0x00, 0x00, 0x00, 0x00], // `
        [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F], // a
        [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1E], // b
        [0x00, 0x00, 0x0E, 0x10, 0x10, 0x11, 0x0E], // c
        [0x01, 0x01, 0x0D, 0x13, 0x11, 0x11, 0x0F], // d
        [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E], // e
        [0x06, 0x09, 0x08, 0x1C, 0x08, 0x08, 0x08], // f
        [0x00, 0x00, 0x0F, 0x11, 0x0F, 0x01, 0x0E], // g (with its descender: DESCENDING)
        [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11], // h
        [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E], // i
        [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0C], // j
        [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12], // k
        [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E], // l
        [0x00, 0x00, 0x1A, 0x15, 0x15, 0x11, 0x11], // m
        [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11], // n
        [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E], // o
        [0x00, 0x00, 0x1E, 0x11, 0x1E, 0x10, 0x10], // p
        [0x00, 0x00, 0x0D, 0x13, 0x0F, 0x01, 0x01], // q
        [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10], // r
        [0x00, 0x00, 0x0E, 0x10, 0x0E, 0x01, 0x1E], // s
        [0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06], // t
        [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0D], // u
        [0x00, 0x00, 0x11, 0x11, 0x11, 0x0A, 0x04], // v
        [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A], // w
        [0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11], // x
        [0x00, 0x00, 0x11, 0x11, 0x0F, 0x01, 0x0E], // y
        [0x00, 0x00, 0x1F, 0x02, 0x04, 0x08, 0x1F], // z
        [0x02, 0x04, 0x04, 0x08, 0x04, 0x04, 0x02], // {
        [0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04], // |
        [0x08, 0x04, 0x04, 0x02, 0x04, 0x04, 0x08], // }
        [0x00, 0x00, 0x08, 0x15, 0x02, 0x00, 0x00], // ~
    ];

    /// The letters that reach below the line, drawn nine rows tall.
    #[rustfmt::skip]
    const DESCENDING: [(char, [u8; 9]); 5] = [
        ('g', [0x00, 0x00, 0x0F, 0x11, 0x11, 0x11, 0x0F, 0x01, 0x0E]),
        ('j', [0x02, 0x00, 0x06, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C]),
        ('p', [0x00, 0x00, 0x1E, 0x11, 0x11, 0x11, 0x1E, 0x10, 0x10]),
        ('q', [0x00, 0x00, 0x0F, 0x11, 0x11, 0x11, 0x0F, 0x01, 0x01]),
        ('y', [0x00, 0x00, 0x11, 0x11, 0x11, 0x11, 0x0F, 0x01, 0x0E]),
    ];

    /// A few characters beyond ASCII that the bar shows: the changed mark, an ellipsis, a
    /// middle dot.
    #[rustfmt::skip]
    const MORE: [(char, [u8; 7]); 3] = [
        ('\u{2022}', [0x00, 0x00, 0x0E, 0x0E, 0x0E, 0x00, 0x00]),
        ('\u{2026}', [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x15]),
        ('\u{b7}', [0x00, 0x00, 0x00, 0x0C, 0x0C, 0x00, 0x00]),
    ];

    /// A letter with an accent as the letter (the display has none of its own), the dashes
    /// and the curly quotes as ASCII's; another character a question mark.
    fn plain(c: char) -> char {
        match c {
            'À'..='Å' => 'A',
            'Ç' => 'C',
            'È'..='Ë' => 'E',
            'Ì'..='Ï' => 'I',
            'Ñ' => 'N',
            'Ò'..='Ö' | 'Ø' => 'O',
            'Ù'..='Ü' => 'U',
            'Ý' => 'Y',
            'à'..='å' => 'a',
            'ç' => 'c',
            'è'..='ë' => 'e',
            'ì'..='ï' => 'i',
            'ñ' => 'n',
            'ò'..='ö' | 'ø' => 'o',
            'ù'..='ü' => 'u',
            'ý' | 'ÿ' => 'y',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201c}' | '\u{201d}' => '"',
            ' '..='~' => c,
            _ => '?',
        }
    }

    /// A character's nine rows (the last two below the line).
    pub fn glyph(c: char) -> [u8; 9] {
        if let Some((_, rows)) = MORE.iter().find(|(k, _)| *k == c) {
            return seven(*rows);
        }
        let c = plain(c);
        if let Some((_, rows)) = DESCENDING.iter().find(|(k, _)| *k == c) {
            return *rows;
        }
        seven(ASCII[(c as usize) - 0x20])
    }

    fn seven(rows: [u8; 7]) -> [u8; 9] {
        let mut out = [0; 9];
        out[..7].copy_from_slice(&rows);
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Every printable character has its own dots (none but the space's blank, none wider
        /// than five), and a name's other characters are shown as near as the display can.
        #[test]
        fn every_character_has_its_dots() {
            for c in ' '..='~' {
                let g = glyph(c);
                assert!(g.iter().all(|r| *r < 0x20), "{c:?} is five dots wide");
                assert_eq!(g.iter().all(|r| *r == 0), c == ' ', "{c:?}");
            }
            for (c, like) in [
                ('é', 'e'),
                ('Ö', 'O'),
                ('\u{2019}', '\''),
                ('\u{2014}', '-'),
                ('\u{df}', '?'),
            ] {
                assert_eq!(glyph(c), glyph(like), "{c:?}");
            }
            // The descenders reach below the line; a capital does not.
            assert!(glyph('g')[7..].iter().any(|r| *r != 0));
            assert!(glyph('G')[7..].iter().all(|r| *r == 0));
            // Each character is its own.
            let mut seen = std::collections::HashSet::new();
            for c in '!'..='~' {
                assert!(seen.insert(glyph(c)), "{c:?} is drawn as another is");
            }
        }
    }
}
