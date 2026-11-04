//! Clipped, merged changes, from the CA-72's renderer and window (R44, R58, R59).

/// Pixel edges: the right and bottom are past the rectangle. Reversed or equal edges are
/// empty. Negative and off-frame coordinates are allowed until clipped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn intersection(self, bounds: Self) -> Option<Self> {
        let r = Self::new(
            self.left.max(bounds.left),
            self.top.max(bounds.top),
            self.right.min(bounds.right),
            self.bottom.min(bounds.bottom),
        );
        (!r.is_empty()).then_some(r)
    }

    /// The smallest rectangle round two non-empty rectangles; an empty one contributes nothing.
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self::new(
            self.left.min(other.left),
            self.top.min(other.top),
            self.right.max(other.right),
            self.bottom.max(other.bottom),
        )
    }

    pub(crate) fn pixmap(p: &crate::tiny_skia::Pixmap) -> Self {
        // tiny-skia limits each dimension to a positive i32.
        Self::new(0, 0, p.width() as i32, p.height() as i32)
    }

    pub(crate) fn placed(p: &crate::tiny_skia::Pixmap, at: (i32, i32)) -> Self {
        Self::new(
            at.0,
            at.1,
            at.0.saturating_add(p.width() as i32),
            at.1.saturating_add(p.height() as i32),
        )
    }

    fn near(self, other: Self, gap: u32) -> bool {
        let gap = i64::from(gap);
        i64::from(self.left) <= i64::from(other.right) + gap
            && i64::from(other.left) <= i64::from(self.right) + gap
            && i64::from(self.top) <= i64::from(other.bottom) + gap
            && i64::from(other.top) <= i64::from(self.bottom) + gap
    }
}

/// Separate changes, with overlapping or touching rectangles joined. The bounding boxes may
/// include unchanged pixels, which must also be rebuilt. No pixel belongs to two rectangles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Damage {
    rects: Vec<Rect>,
}

impl Damage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn full(bounds: Rect) -> Self {
        let mut d = Self::new();
        d.add(bounds);
        d
    }

    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
    pub fn clear(&mut self) {
        self.rects.clear();
    }

    pub fn add(&mut self, rect: Rect) {
        if !rect.is_empty() {
            self.rects.push(rect);
            self.join(0);
        }
    }

    /// Clip before merging (R58). Prefer this to adding off-component changes then clipping
    /// their bounding box: that box may already have swallowed the space between changes.
    pub fn add_clipped(&mut self, rect: Rect, bounds: Rect) {
        if let Some(rect) = rect.intersection(bounds) {
            self.add(rect);
        }
    }

    pub fn clipped(&self, bounds: Rect) -> Self {
        let mut d = Self::new();
        for &r in &self.rects {
            d.add_clipped(r, bounds);
        }
        d
    }

    /// Also join changes within `gap` pixels, and use one bounding box beyond `maximum`
    /// places (zero means one). These are the caller's cost choices, not panel dimensions.
    pub fn merge(&mut self, gap: u32, maximum: usize) {
        self.join(gap);
        if self.rects.len() > maximum.max(1) {
            self.rects = self
                .rects
                .iter()
                .copied()
                .reduce(Rect::union)
                .into_iter()
                .collect();
        }
    }

    fn join(&mut self, gap: u32) {
        'again: loop {
            for i in 0..self.rects.len() {
                for j in i + 1..self.rects.len() {
                    if self.rects[i].near(self.rects[j], gap) {
                        self.rects[i] = self.rects[i].union(self.rects[j]);
                        self.rects.swap_remove(j);
                        continue 'again;
                    }
                }
            }
            break;
        }
    }

    /// Damage for a window buffer (R58, R59): partial work only when it contains the complete
    /// last frame, its age is one and changes are known. The adapter passes `complete = false`
    /// after creation, resizing, a failed present or a host repaint. It records success only
    /// after presentation succeeds. No softbuffer or window types are needed here.
    pub fn for_buffer(bounds: Rect, changes: Option<&Self>, age: u8, complete: bool) -> Self {
        match changes {
            Some(d) if age == 1 && complete => d.clipped(bounds),
            _ => Self::full(bounds),
        }
    }
}
