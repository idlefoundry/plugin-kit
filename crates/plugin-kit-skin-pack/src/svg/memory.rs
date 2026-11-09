//! A drawing's buffers, bounded before resvg allocates any (decisions.md R-SKIN-PACKS). resvg
//! 0.45 allocates a pixmap for each of these, and an allocation that fails aborts the process
//! (it is not a panic, and no thread contains it):
//!
//! - a group drawn on a layer of its own (an opacity, a clip, a mask, a filter): its bounding
//!   box, cut to five times the target's size each way;
//! - a pattern: one tile at its transformed size, however little of it is seen, for each path
//!   it paints;
//! - a filter: its whole region (not cut to the target), once for each primitive's result, all
//!   kept until the filter ends, and each primitive's inputs, copies and scratch space;
//!   `feImage` draws its elements into a region of its own;
//! - a clip and a mask: copies of the layer they cut, one more for each clip or mask they are
//!   themselves cut by;
//! - an embedded picture: decoded at its own size.
//!
//! Every buffer is counted at the size resvg gives it (a layer at most the cut it is given), and
//! all of them are added up, as if none were freed: a bound on what drawing can hold at once.
//! Coordinates far outside a drawing's integer range are refused too, before tiny-skia's
//! integer rectangles see them.

use resvg::tiny_skia::{NonZeroRect, Transform};
use resvg::usvg;

use crate::Limits;

/// The bytes of a drawing's buffers drawn with its longer side `side` pixels (`Err` with why it
/// is past `limits`).
pub(super) fn check(tree: &usvg::Tree, side: u32, limits: &Limits) -> Result<u64, String> {
    let size = tree.size();
    let k = side as f32 / size.width().max(size.height());
    let canvas = (
        (size.width() * k).ceil().max(1.0),
        (size.height() * k).ceil().max(1.0),
    );
    if !(canvas.0.is_finite() && canvas.1.is_finite()) {
        return Err("the drawing has no size to draw at".into());
    }
    check_at(
        tree,
        Transform::from_scale(k, k),
        (canvas.0 as u32, canvas.1 as u32),
        limits,
    )
}

/// The bytes of a drawing's buffers drawn by `transform` into a target `canvas` pixels (`Err`
/// with why it is past `limits`).
pub(super) fn check_at(
    tree: &usvg::Tree,
    transform: Transform,
    canvas: (u32, u32),
    limits: &Limits,
) -> Result<u64, String> {
    let mut buffers = Buffers {
        limits,
        bytes: 0,
        nodes: 0,
        cut: (canvas.0.saturating_mul(5), canvas.1.saturating_mul(5)),
    };
    let canvas = buffers.size("the drawing", canvas.0 as f32, canvas.1 as f32)?;
    buffers.add("the drawing", canvas, 1)?;
    buffers.nodes(tree.root(), transform, canvas, false)?;
    Ok(buffers.bytes)
}

/// Integer coordinates past this are refused: tiny-skia's rectangles hold `i32`, and resvg
/// rounds, pads, shifts and subtracts them.
const RANGE: f32 = (i32::MAX / 8) as f32;

/// RGBA, premultiplied.
const BYTES_A_PIXEL: u64 = 4;

struct Buffers<'a> {
    limits: &'a Limits,
    bytes: u64,
    nodes: usize,
    /// The largest layer resvg draws (its `max_bbox`): five times the target's size each way, or
    /// an `feImage`'s region.
    cut: (u32, u32),
}

impl Buffers<'_> {
    /// A buffer `w` by `h` pixels, within the limit for one buffer.
    fn size(&self, what: &str, w: f32, h: f32) -> Result<(u32, u32), String> {
        if !w.is_finite() || !h.is_finite() || w < 0.0 || h < 0.0 {
            return Err(format!("{what} has no size to draw at"));
        }
        let (w, h) = (f64::from(w).ceil().max(1.0), f64::from(h).ceil().max(1.0));
        if w * h > self.limits.svg_buffer_pixels as f64 {
            return Err(format!(
                "{what} needs a drawing buffer of {w} by {h} pixels, more than {} pixels",
                self.limits.svg_buffer_pixels
            ));
        }
        Ok((w as u32, h as u32))
    }

    /// `rect` placed by `t`, within the drawing's integer range.
    fn placed(&self, what: &str, rect: NonZeroRect, t: Transform) -> Result<NonZeroRect, String> {
        let r = rect
            .transform(t)
            .ok_or_else(|| format!("{what} has no size to draw at"))?;
        if [r.x(), r.y(), r.right(), r.bottom()]
            .iter()
            .any(|n| !n.is_finite() || n.abs() > RANGE)
        {
            return Err(format!("{what} lies outside the drawing's integer range"));
        }
        Ok(r)
    }

    /// `rect` placed by `t`, as a buffer.
    fn rect(&self, what: &str, rect: NonZeroRect, t: Transform) -> Result<(u32, u32), String> {
        let r = self.placed(what, rect, t)?;
        self.size(what, r.width(), r.height())
    }

    /// `copies` buffers of `size` added to the drawing's.
    fn add(&mut self, what: &str, (w, h): (u32, u32), copies: u64) -> Result<(), String> {
        let bytes = u64::from(w)
            .checked_mul(u64::from(h))
            .and_then(|n| n.checked_mul(BYTES_A_PIXEL))
            .and_then(|n| n.checked_mul(copies))
            .and_then(|n| self.bytes.checked_add(n));
        match bytes {
            Some(n) if n <= self.limits.svg_buffer_bytes => {
                self.bytes = n;
                Ok(())
            }
            _ => Err(format!(
                "{what} takes the drawing's buffers past {} MB together",
                self.limits.svg_buffer_bytes >> 20
            )),
        }
    }

    /// One more element visited (a reference's each time it is used).
    fn visit(&mut self) -> Result<(), String> {
        self.nodes += 1;
        if self.nodes > self.limits.svg_nodes {
            return Err(format!(
                "more than {} elements drawn, its buffers counted",
                self.limits.svg_nodes
            ));
        }
        Ok(())
    }

    /// A group's children, drawn by `t` onto a layer `canvas` pixels.
    fn nodes(
        &mut self,
        group: &usvg::Group,
        t: Transform,
        canvas: (u32, u32),
        in_pattern: bool,
    ) -> Result<(), String> {
        self.visit()?;
        for node in group.children() {
            self.visit()?;
            match node {
                usvg::Node::Group(g) => self.group(g, t, canvas, in_pattern)?,
                usvg::Node::Path(p) => self.path(p, t, in_pattern)?,
                usvg::Node::Image(i) => {
                    // Decoded at its own size (up to 16 bits a channel), then made a pixmap: the
                    // size read from the PNG's header (one that cannot be read is not drawn).
                    if let usvg::ImageKind::PNG(data) = i.kind() {
                        let be = |at: usize| {
                            data.get(at..at + 4)
                                .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                        };
                        let size =
                            self.size("an embedded picture", be(16) as f32, be(20) as f32)?;
                        self.add("an embedded picture", size, 3)?;
                    }
                }
                // (Refused by the scan: a pack's legends are its layout's.)
                usvg::Node::Text(_) => return Err("text in a pack's drawing".into()),
            }
        }
        Ok(())
    }

    /// A path's paints: a pattern's tile, drawn for each.
    fn path(&mut self, p: &usvg::Path, t: Transform, in_pattern: bool) -> Result<(), String> {
        for paint in [p.fill().map(|f| f.paint()), p.stroke().map(|s| s.paint())]
            .into_iter()
            .flatten()
        {
            if let usvg::Paint::Pattern(pat) = paint {
                // (Refused by the cost, too.)
                if in_pattern {
                    return Err("a pattern inside a pattern".into());
                }
                // resvg's `render_pattern_pixmap`: the tile at the scale of the pattern's
                // transform after the path's (rounded there; rounded up here), its elements
                // drawn at that scale.
                let (sx, sy) = t.pre_concat(pat.transform()).get_scale();
                let r = pat.rect();
                let size = self.size("a pattern's tile", r.width() * sx, r.height() * sy)?;
                self.add("a pattern's tile", size, 1)?;
                self.nodes(pat.root(), Transform::from_scale(sx, sy), size, true)?;
            }
        }
        Ok(())
    }

    /// A group: its own layer if it has one, its children, its filters, clip and mask.
    fn group(
        &mut self,
        group: &usvg::Group,
        t: Transform,
        canvas: (u32, u32),
        in_pattern: bool,
    ) -> Result<(), String> {
        let t = t.pre_concat(group.transform());
        let canvas = if group.should_isolate() {
            // resvg's `render_group`: the layer's box rounded out (by 2 pixels each side unless
            // filtered), cut to `max_bbox`. Where it is only moves it; its size is what counts.
            let r = self.placed("a group's layer", group.layer_bounding_box(), t)?;
            let pad = if group.filters().is_empty() { 4.0 } else { 0.0 };
            let w = (r.width().ceil() + pad).min(self.cut.0 as f32);
            let h = (r.height().ceil() + pad).min(self.cut.1 as f32);
            let size = self.size("a group's layer", w, h)?;
            self.add("a group's layer", size, 1)?;
            size
        } else {
            canvas
        };
        self.nodes(group, t, canvas, in_pattern)?;
        for filter in group.filters() {
            // Every primitive's result is the filter's region (not cut), and is kept; an input
            // may be the layer itself. Eight buffers a primitive bound its result, its inputs
            // and their colour conversions, its copies (a drop shadow's three) and a blur's
            // scratch (a float a channel).
            let region = self.rect("a filter's region", filter.rect(), t)?;
            let each = self.size(
                "a filter's buffers",
                region.0.max(canvas.0) as f32,
                region.1.max(canvas.1) as f32,
            )?;
            let (sx, sy) = t.get_scale();
            for primitive in filter.primitives() {
                self.placed("a filter primitive's region", primitive.rect(), t)?;
                match primitive.kind() {
                    usvg::filter::Kind::GaussianBlur(b) => {
                        blur(b.std_dev_x().get() * sx, b.std_dev_y().get() * sy)?;
                    }
                    usvg::filter::Kind::DropShadow(d) => {
                        blur(d.std_dev_x().get() * sx, d.std_dev_y().get() * sy)?;
                        offset(d.dx() * sx, d.dy() * sy)?;
                    }
                    usvg::filter::Kind::Offset(o) => offset(o.dx() * sx, o.dy() * sy)?,
                    usvg::filter::Kind::Image(image) => {
                        // (Refused by the scan.) Its elements drawn into a buffer the region's
                        // size, at the filter's scale, their own layers cut to that region.
                        let cut = std::mem::replace(&mut self.cut, region);
                        let drawn = self.nodes(
                            image.root(),
                            Transform::from_scale(sx, sy),
                            region,
                            in_pattern,
                        );
                        self.cut = cut;
                        drawn?;
                    }
                    _ => {}
                }
                self.add("a filter's buffers", each, 8)?;
            }
        }
        if let Some(clip) = group.clip_path() {
            self.clip(clip, t, canvas, in_pattern)?;
        }
        if let Some(mask) = group.mask() {
            self.mask(mask, t, canvas, in_pattern)?;
        }
        Ok(())
    }

    /// resvg's `clip::apply`: a pixmap the layer's size and the alpha mask made from it, and
    /// again for the clip's own clip.
    fn clip(
        &mut self,
        clip: &usvg::ClipPath,
        t: Transform,
        canvas: (u32, u32),
        in_pattern: bool,
    ) -> Result<(), String> {
        self.add("a clip", canvas, 2)?;
        self.clip_nodes(
            clip.root(),
            t.pre_concat(clip.transform()),
            canvas,
            in_pattern,
        )?;
        if let Some(c) = clip.clip_path() {
            self.clip(c, t, canvas, in_pattern)?;
        }
        Ok(())
    }

    /// A clip's children: drawn straight into its pixmap, but for a group with a clip of its
    /// own, drawn on a pixmap the layer's size first (resvg's `clip_group`).
    fn clip_nodes(
        &mut self,
        group: &usvg::Group,
        t: Transform,
        canvas: (u32, u32),
        in_pattern: bool,
    ) -> Result<(), String> {
        self.visit()?;
        for node in group.children() {
            self.visit()?;
            match node {
                usvg::Node::Group(g) => {
                    let t = t.pre_concat(g.transform());
                    if let Some(c) = g.clip_path() {
                        self.add("a clipped group in a clip", canvas, 1)?;
                        self.clip(c, t, canvas, in_pattern)?;
                    }
                    self.clip_nodes(g, t, canvas, in_pattern)?;
                }
                usvg::Node::Path(p) => self.path(p, t, in_pattern)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// resvg's `mask::apply`: a pixmap the layer's size, an alpha mask for its region and the
    /// mask made from it (each counted as a pixmap), its children drawn into it, and again for
    /// the mask's own mask.
    fn mask(
        &mut self,
        mask: &usvg::Mask,
        t: Transform,
        canvas: (u32, u32),
        in_pattern: bool,
    ) -> Result<(), String> {
        self.placed("a mask's region", mask.rect(), t)?;
        self.add("a mask", canvas, 3)?;
        self.nodes(mask.root(), t, canvas, in_pattern)?;
        if let Some(m) = mask.mask() {
            self.mask(m, t, canvas, in_pattern)?;
        }
        Ok(())
    }
}

/// A blur's deviation in pixels, each way: within what resvg's box blur sizes as integers.
fn blur(x: f32, y: f32) -> Result<(), String> {
    if [x, y].iter().any(|v| !v.is_finite() || v.abs() > 4096.0) {
        return Err("a blur of more than 4096 pixels where it is drawn".into());
    }
    Ok(())
}

/// A filter's offset in pixels, each way: within the drawing's integer range.
fn offset(x: f32, y: f32) -> Result<(), String> {
    if [x, y].iter().any(|v| !v.is_finite() || v.abs() > RANGE) {
        return Err("a filter's offset lies outside the drawing's integer range".into());
    }
    Ok(())
}
