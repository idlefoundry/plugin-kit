//! Menu rows and placement, from the CA-72's and CA-74's `learn::Menu` (R34,
//! R51). The caller measures and draws every row, including titles and group heads,
//! and translates its hit test to a row index. This module supplies no lettering or sizes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuRow<A> {
    Title(String),
    Head(String),
    Item {
        label: String,
        action: A,
        enabled: bool,
        marked: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu<A> {
    rows: Vec<MenuRow<A>>,
    hover: Option<usize>,
}

/// A measured menu uniformly reduced only when it exceeds the available area.
/// The caller draws with `scale` and uses the same transform for its hit test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub bounds: [f64; 4],
    pub scale: f64,
}

impl<A> Menu<A> {
    pub fn new(rows: Vec<MenuRow<A>>) -> Self {
        Self { rows, hover: None }
    }

    pub fn rows(&self) -> &[MenuRow<A>] {
        &self.rows
    }

    /// Titles, heads, disabled items and absent rows cannot be activated. A marked
    /// item may still be enabled; its current mark is independent of its ability to act.
    pub fn choose(&self, row: usize) -> Option<&A> {
        match self.rows.get(row)? {
            MenuRow::Item {
                action,
                enabled: true,
                ..
            } => Some(action),
            _ => None,
        }
    }

    pub fn hover(&self) -> Option<usize> {
        self.hover
    }

    /// Only selectable rows receive a hover, so a heading cannot look actionable.
    pub fn set_hover(&mut self, row: Option<usize>) {
        self.hover = row.filter(|&row| self.choose(row).is_some());
    }

    /// Fit the caller's complete measurement (margins included) inside `bounds`,
    /// retaining `at` where possible. Positive finite dimensions and a non-empty
    /// finite area are required. No measurement or row index is changed here.
    pub fn fit(
        &self,
        at: (f64, f64),
        bounds: [f64; 4],
        measured_size: (f64, f64),
    ) -> Result<Placement, &'static str> {
        let [left, top, right, bottom] = bounds;
        let (width, height) = measured_size;
        let available_w = right - left;
        let available_h = bottom - top;
        if ![
            left,
            top,
            right,
            bottom,
            at.0,
            at.1,
            width,
            height,
            available_w,
            available_h,
        ]
        .into_iter()
        .all(f64::is_finite)
            || available_w <= 0.0
            || available_h <= 0.0
            || width <= 0.0
            || height <= 0.0
        {
            return Err("A menu needs finite measurements and a non-empty area");
        }
        let scale = (available_w / width).min(available_h / height).min(1.0);
        if scale <= 0.0 {
            return Err("The measured menu cannot fit this area");
        }
        let w = (width * scale).min(available_w);
        let h = (height * scale).min(available_h);
        let x = at.0.max(left).min(right - w).max(left);
        let y = at.1.max(top).min(bottom - h).max(top);
        Ok(Placement {
            bounds: [x, y, (x + w).min(right), (y + h).min(bottom)],
            scale,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu<u8> {
        Menu::new(vec![
            MenuRow::Title("SETTINGS".into()),
            MenuRow::Head("GROUP".into()),
            MenuRow::Item {
                label: "CURRENT".into(),
                action: 1,
                enabled: true,
                marked: true,
            },
            MenuRow::Item {
                label: "UNAVAILABLE".into(),
                action: 2,
                enabled: false,
                marked: false,
            },
            MenuRow::Item {
                label: "OTHER".into(),
                action: 3,
                enabled: true,
                marked: false,
            },
        ])
    }

    #[test]
    fn titles_heads_and_disabled_items_are_never_chosen() {
        let mut m = menu();
        for (row, want) in [
            (0, None),
            (1, None),
            (2, Some(&1)),
            (3, None),
            (4, Some(&3)),
            (5, None),
        ] {
            assert_eq!(m.choose(row), want);
            m.set_hover(Some(row));
            assert_eq!(m.hover(), want.map(|_| row));
        }
        m.set_hover(None);
        assert_eq!(m.hover(), None);
        assert!(matches!(
            &m.rows()[2],
            MenuRow::Item {
                marked: true,
                enabled: true,
                ..
            }
        ));
    }

    #[test]
    fn a_menu_fits_at_every_edge_without_changing_rows() {
        let m = menu();
        let area = [10.0, 20.0, 410.0, 320.0];
        for at in [(0.0, 0.0), (400.0, 310.0), (20.0, 40.0), (900.0, -100.0)] {
            let p = m.fit(at, area, (130.0, 120.0)).expect("fits");
            assert_eq!(p.scale, 1.0);
            let [x, y, right, bottom] = p.bounds;
            assert!(x >= area[0] && y >= area[1] && right <= area[2] && bottom <= area[3]);
            assert_eq!((right - x, bottom - y), (130.0, 120.0));
        }
        assert_eq!(
            m.fit((20.0, 40.0), area, (130.0, 120.0))
                .expect("fits")
                .bounds,
            [20.0, 40.0, 150.0, 160.0]
        );
        assert_eq!(m, menu());
    }

    #[test]
    fn an_oversized_menu_is_uniformly_reduced() {
        let m = menu();
        for (size, scale, expected) in [
            ((200.0, 400.0), 0.5, [100.0, 0.0, 200.0, 200.0]),
            ((400.0, 100.0), 0.5, [0.0, 150.0, 200.0, 200.0]),
        ] {
            let p = m
                .fit((199.0, 199.0), [0.0, 0.0, 200.0, 200.0], size)
                .expect("fits");
            assert_eq!(
                p,
                Placement {
                    bounds: expected,
                    scale
                }
            );
        }
    }

    #[test]
    fn invalid_measurements_are_errors() {
        let m = menu();
        for (at, bounds, size) in [
            ((0.0, 0.0), [0.0, 0.0, 0.0, 2.0], (1.0, 1.0)),
            ((0.0, 0.0), [2.0, 0.0, 1.0, 2.0], (1.0, 1.0)),
            ((0.0, 0.0), [0.0, 0.0, 2.0, 2.0], (f64::NAN, 1.0)),
            ((0.0, f64::INFINITY), [0.0, 0.0, 2.0, 2.0], (1.0, 1.0)),
            ((0.0, 0.0), [0.0, 0.0, 2.0, 2.0], (0.0, 1.0)),
        ] {
            assert!(m.fit(at, bounds, size).is_err());
        }
    }
}
