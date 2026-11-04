use crate::{Rect, compose, tiny_skia::Pixmap};

/// An owned picture with runs of non-transparent pixels, from the CA-72's strip (R44, R48).
/// Built once, then laid only where it has pixels. Its pixels cannot be changed without
/// rebuilding the runs. Translation and clipping are in the target's pixels.
#[derive(Clone, Debug)]
pub struct Sparse {
    pixmap: Pixmap,
    runs: Vec<Rect>,
}

impl Sparse {
    pub fn new(pixmap: Pixmap) -> Self {
        let w = pixmap.width() as usize;
        let mut runs = Vec::new();
        for (y, row) in pixmap.pixels().chunks_exact(w).enumerate() {
            let mut x = 0;
            while x < w {
                if row[x].alpha() == 0 {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < w && row[x].alpha() != 0 {
                    x += 1;
                }
                runs.push(Rect::new(start as i32, y as i32, x as i32, y as i32 + 1));
            }
        }
        Self { pixmap, runs }
    }

    pub fn from_pixmap(pixmap: &Pixmap) -> Self {
        Self::new(pixmap.clone())
    }
    pub fn pixmap(&self) -> &Pixmap {
        &self.pixmap
    }

    /// Source-over, as [`crate::over`], without visiting transparent runs.
    pub fn over(&self, target: &mut Pixmap, at: (i32, i32), clip: Rect) {
        let Some(r) = compose::place(target, &self.pixmap, at, clip) else {
            return;
        };
        let top = i64::from(r.top) - i64::from(at.1);
        let first = self.runs.partition_point(|run| i64::from(run.top) < top);
        for run in &self.runs[first..] {
            let y = i64::from(run.top) + i64::from(at.1);
            if y >= i64::from(r.bottom) {
                break;
            }
            let left = (i64::from(run.left) + i64::from(at.0)).max(i64::from(r.left));
            let right = (i64::from(run.right) + i64::from(at.0)).min(i64::from(r.right));
            if right <= left {
                continue;
            }
            let row = Rect::new(left as i32, y as i32, right as i32, y as i32 + 1);
            compose::blend(
                compose::target_row(target, row, y as i32),
                compose::source_row(&self.pixmap, at, row, y as i32),
            );
        }
    }
}
