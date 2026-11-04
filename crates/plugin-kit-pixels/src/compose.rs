use crate::{Damage, Rect, tiny_skia::Pixmap};

pub(crate) fn place(dst: &Pixmap, src: &Pixmap, at: (i32, i32), clip: Rect) -> Option<Rect> {
    clip.intersection(Rect::pixmap(dst))?
        .intersection(Rect::placed(src, at))
}

pub(crate) fn source_row(src: &Pixmap, at: (i32, i32), r: Rect, y: i32) -> &[u8] {
    let start = ((i64::from(y) - i64::from(at.1)) as usize * src.width() as usize
        + (i64::from(r.left) - i64::from(at.0)) as usize)
        * 4;
    &src.data()[start..start + (r.right - r.left) as usize * 4]
}

pub(crate) fn target_row(dst: &mut Pixmap, r: Rect, y: i32) -> &mut [u8] {
    let start = (y as usize * dst.width() as usize + r.left as usize) * 4;
    &mut dst.data_mut()[start..start + (r.right - r.left) as usize * 4]
}

/// Replace pixels inside `clip`, with `src`'s top left at `at` in `dst`'s pixels. Transparent
/// source pixels replace too. From the CA-72's editor `compose` and `paste` (R58).
pub fn copy(dst: &mut Pixmap, src: &Pixmap, at: (i32, i32), clip: Rect) {
    let Some(r) = place(dst, src, at, clip) else {
        return;
    };
    for y in r.top..r.bottom {
        target_row(dst, r, y).copy_from_slice(source_row(src, at, r, y));
    }
}

/// Source-over inside `clip`, with `src`'s top left at `at`. Both pixmaps are premultiplied.
/// From the CA-72's `glow::lay` and editor composition; [`crate::Sparse`] uses the same
/// rounded integer arithmetic, so choosing a sparse picture changes no pixel.
pub fn over(dst: &mut Pixmap, src: &Pixmap, at: (i32, i32), clip: Rect) {
    let Some(r) = place(dst, src, at, clip) else {
        return;
    };
    for y in r.top..r.bottom {
        blend(target_row(dst, r, y), source_row(src, at, r, y));
    }
}

pub(crate) fn blend(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        match s[3] {
            0 => {}
            255 => d.copy_from_slice(s),
            alpha => {
                let keep = 255 - u16::from(alpha);
                for c in 0..4 {
                    d[c] = (u16::from(s[c]) + (u16::from(d[c]) * keep + 127) / 255) as u8;
                }
            }
        }
    }
}

/// Turn only the damage into window words `0xffRRGGBB`, a row at a time (CA-72's window
/// `fill_places`, R48, R58). Coordinates are clipped to `source`; the rest of `target`,
/// including any trailing storage, is untouched. A short target fails before any write.
///
/// As in the source editor, RGB bytes are kept and alpha is forced opaque, for compositing
/// desktops too (R38). Compose onto an opaque background before conversion; this neither
/// demultiplies transparent pixels nor chooses a background colour for the plug-in.
pub fn convert(source: &Pixmap, target: &mut [u32], damage: &Damage) -> Result<(), &'static str> {
    let required = source.pixels().len();
    if target.len() < required {
        return Err("the window buffer is smaller than the frame");
    }
    let w = source.width() as usize;
    for r in damage.clipped(Rect::pixmap(source)).rects() {
        for y in r.top..r.bottom {
            let start = y as usize * w + r.left as usize;
            let end = y as usize * w + r.right as usize;
            for (d, s) in target[start..end]
                .iter_mut()
                .zip(source.data()[start * 4..end * 4].chunks_exact(4))
            {
                *d = 0xff00_0000
                    | (u32::from(s[0]) << 16)
                    | (u32::from(s[1]) << 8)
                    | u32::from(s[2]);
            }
        }
    }
    Ok(())
}
