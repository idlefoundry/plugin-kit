//! What a pack's drawing costs (decisions.md R-SKIN-PACKS): `cargo run --release -p skin-pack
//! --example svg_cost -- <drawing.svg> [side]`. Prints its counts and its work against the
//! limits, costed and drawn `side` pixels across (the size the plug-in uses it at: 3000 for the
//! panel's drawing, a picture's slot's width; without it, its own size, at most 3000), and how
//! long it takes to draw (waited for up to a minute); then the same with each of its costly kinds
//! of part left out, to find what costs.

use std::time::Instant;

use plugin_kit_skin_pack::Limits;
use plugin_kit_skin_pack::svg::{self, Drawn};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("svg_cost <drawing.svg> [side]");
        std::process::exit(2);
    };
    let side: Option<u32> = args.get(1).and_then(|s| s.parse().ok());
    let text = std::fs::read_to_string(path).expect("the drawing");
    let limits = Limits::default();
    let time = |label: &str, text: &str| {
        let t = Instant::now();
        match svg::parse_at(text.as_bytes(), &limits, side) {
            Ok((tree, cost)) => {
                let parsed = t.elapsed().as_secs_f64() * 1000.0;
                let own = tree.size();
                let at = side.unwrap_or_else(|| {
                    (own.width().max(own.height()).ceil() as u32).min(limits.svg_test_side)
                });
                let drawn = match svg::test_draw(&tree, at, 60_000) {
                    Drawn::In(ms) => format!("{ms} ms"),
                    other => format!("{other:?}"),
                };
                println!("{label:<28} read {parsed:6.0} ms, drawn {drawn:>10}   {cost:?}");
            }
            Err(e) => println!("{label:<28} refused: {e}"),
        }
    };
    time("whole", &text);
    let without = |pat: &str| -> String {
        // Each element whose text starts with `pat` taken out (a rough knife, for measuring).
        let mut out = String::new();
        let mut rest = text.as_str();
        while let Some(i) = rest.find(pat) {
            out.push_str(&rest[..i]);
            let tail = &rest[i..];
            let end = tail.find("/>").map(|e| e + 2).unwrap_or(tail.len());
            rest = &tail[end..];
        }
        out.push_str(rest);
        out
    };
    for (label, pat) in [
        ("without <use>", "<use "),
        ("without <path>", "<path "),
        ("without filtered <rect>", "<rect x='1"),
        ("without <image>", "<image "),
    ] {
        time(label, &without(pat));
    }
}
