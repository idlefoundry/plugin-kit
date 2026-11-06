//! Fuzzing the pack's readers (decisions.md R-SKIN-PACKS): valid inputs mutated thousands of
//! ways (bytes flipped, cut short, copied, spliced; numbers made vast or strange; parts
//! repeated), each read as a pack would be. A reader must never panic, never take long, and
//! always answer (loaded or refused). `SKIN_PACK_FUZZ=<n>` runs `n` cases a reader (300 by
//! default); a case that fails is written to the system's temporary folder to reproduce.
//! `SKIN_PACK_FUZZ_SEED=<u64>` selects another repeatable mutation sequence;
//! `SKIN_PACK_FUZZ_LAST=<file>` writes each case there before it is read, so that a case that
//! aborts the process (which no test can catch) is left there.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use plugin_kit_skin_pack::{Context, Files, Limits, Pack, Report, archive, font, manifest, svg};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn cases() -> usize {
    std::env::var("SKIN_PACK_FUZZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
}

/// `seed` mutated: a few of bytes flipped, a cut, a copy of a span, a splice, a vast number.
fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..1 + rng.below(4) {
        if b.is_empty() {
            b.push(rng.next() as u8);
            continue;
        }
        match rng.below(7) {
            0 => {
                let i = rng.below(b.len());
                b[i] ^= 1 << rng.below(8);
            }
            1 => {
                let i = rng.below(b.len());
                b[i] = rng.next() as u8;
            }
            2 => {
                let at = rng.below(b.len());
                b.truncate(at.max(1));
            }
            3 => {
                let (i, n) = (rng.below(b.len()), 1 + rng.below(64));
                let span: Vec<u8> = b[i..(i + n).min(b.len())].to_vec();
                let at = rng.below(b.len());
                for _ in 0..1 + rng.below(8) {
                    b.splice(at..at, span.iter().copied());
                }
            }
            4 => {
                let at = rng.below(b.len());
                let words: [&[u8]; 8] = [
                    b"1e308",
                    b"-1e308",
                    b"nan",
                    b"inf",
                    b"999999999999",
                    b"-0",
                    b"0.0000001",
                    b"18446744073709551616",
                ];
                b.splice(at..at, words[rng.below(words.len())].iter().copied());
            }
            5 => {
                let i = rng.below(b.len());
                let j = (i + 1 + rng.below(32)).min(b.len());
                b.drain(i..j);
            }
            _ => {
                let at = rng.below(b.len());
                let n = rng.below(16);
                for _ in 0..n {
                    b.insert(at, rng.next() as u8);
                }
            }
        }
    }
    b
}

/// Runs `read` on `cases()` mutations of each seed: it must not panic, nor take more than
/// `most` (a debug build's).
fn fuzz(name: &str, seeds: &[Vec<u8>], most: Duration, read: impl Fn(&[u8])) {
    let seed = std::env::var("SKIN_PACK_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ name.len() as u64 ^ seed);
    let last = std::env::var_os("SKIN_PACK_FUZZ_LAST")
        .map(|p| std::path::PathBuf::from(p).with_extension(name));
    for k in 0..cases() {
        let seed = &seeds[k % seeds.len()];
        let input = mutate(seed, &mut rng);
        if let Some(last) = &last {
            std::fs::write(last, &input).expect("the case written");
        }
        let t = Instant::now();
        let ok = catch_unwind(AssertUnwindSafe(|| read(&input))).is_ok();
        let took = t.elapsed();
        if !ok || took > most {
            let path = std::env::temp_dir().join(format!("skin-pack-fuzz-{name}-{k}.bin"));
            let _ = std::fs::write(&path, &input);
            panic!(
                "{name} case {k}: {} after {took:?}; the input is in {}",
                if ok { "too slow" } else { "panicked" },
                path.display()
            );
        }
    }
}

const SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 200 100' width='200' \
height='100'><defs><linearGradient id='g'><stop offset='0' stop-color='#123'/><stop offset='1' \
stop-color='#abc'/></linearGradient><pattern id='p' width='10' height='10' \
patternUnits='userSpaceOnUse'><rect width='5' height='5'/></pattern><filter id='f'>\
<feGaussianBlur stdDeviation='2'/></filter><g id='t'><circle r='3'/><path d='M0 0 L10 5 Q 20 \
0 30 5 C 40 0 50 10 60 5 Z'/></g></defs><rect width='200' height='100' fill='url(#g)'/>\
<rect width='100' height='50' fill='url(#p)' filter='url(#f)'/><use href='#t' x='20' y='20'/>\
<use href='#t' x='60' y='40'/><g opacity='0.5' clip-path='url(#c)'><clipPath id='c'><rect \
width='50' height='50'/></clipPath><ellipse cx='40' cy='40' rx='30' ry='10'/></g></svg>";

const MANIFEST: &str = "format = 1\nid = \"org.example.fuzz\"\nname = \"FUZZ\"\nversion = \
\"1.0.0\"\nplugin = \"ca-72\"\nlicense = \"CC-BY-4.0 OR LicenseRef-Fuzz\"\nlicense_files = \
[\"LICENSE.txt\"]\n\n[[creators]]\nname = \"A\"\n\n[[assets]]\npath = \"art/a.svg\"\norigin = \
\"generated\"\ntool = \"t\"\nsha256 = \"00\"\n\n[[notices]]\nname = \"n\"\nlicense = \"MIT\"\n\
file = \"LICENSE.txt\"\n\n[declarations]\nrights_to_distribute = true\n\
authorizes_customer_licensing = false\ndeclared_by = \"A\"\ndeclared_on = \"2026-10-16\"\n\n\
[look]\nbase = \"classic\"\n";

fn context() -> Context<'static> {
    Context {
        plugin: "ca-72",
        official: &[],
        official_licence: "GPL-3.0-or-later",
        built_in: false,
        test_drawings: false,
        keys: &[],
        plugin_version: None,
    }
}

fn pack_file() -> Vec<u8> {
    let mut r = Report::default();
    let png = {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, 8, 8);
            e.set_color(png::ColorType::Rgba);
            let mut w = e.write_header().expect("a header");
            w.write_image_data(&[90; 256]).expect("pixels");
        }
        out
    };
    let files = Files::from_memory(
        [
            ("skin.toml", MANIFEST.as_bytes()),
            ("LICENSE.txt", b"terms".as_slice()),
            ("art/a.svg", SVG.as_bytes()),
            ("pictures/p.png", png.as_slice()),
        ],
        &Limits::default(),
        &mut r,
    );
    archive::write(&files).expect("a pack file")
}

#[test]
fn pack_files_never_panic_or_hang() {
    let seeds = vec![pack_file()];
    fuzz("archive", &seeds, Duration::from_secs(5), |b| {
        let mut r = Report::default();
        let files = archive::read(b, &Limits::default(), &mut r);
        let _ = Pack::check(files, r, &Limits::default(), &context());
    });
}

/// The files inside a valid pack file mutated (the archive itself kept valid), so that what is
/// read reaches the checks behind the archive's.
#[test]
fn packs_mutated_inside_never_panic_or_hang() {
    let base = pack_file();
    let mut rng = Rng(77);
    let mut r = Report::default();
    let files = archive::read(&base, &Limits::default(), &mut r);
    let names: Vec<String> = files.paths().map(str::to_owned).collect();
    let seeds: Vec<Vec<u8>> = (0..cases().min(2000))
        .map(|_| {
            let victim = &names[rng.below(names.len())];
            let changed: Vec<(String, Vec<u8>)> = files
                .iter()
                .map(|(p, b)| {
                    let b = if p == victim {
                        mutate(b, &mut rng)
                    } else {
                        b.to_vec()
                    };
                    (p.to_owned(), b)
                })
                .collect();
            let mut r = Report::default();
            let f = Files::from_memory(
                changed.iter().map(|(p, b)| (p.as_str(), b.as_slice())),
                &Limits::default(),
                &mut r,
            );
            archive::write(&f).expect("a pack file")
        })
        .collect();
    for (k, seed) in seeds.iter().enumerate() {
        let t = Instant::now();
        let ok = catch_unwind(AssertUnwindSafe(|| {
            let mut r = Report::default();
            let files = archive::read(seed, &Limits::default(), &mut r);
            let _ = Pack::check(files, r, &Limits::default(), &context());
        }))
        .is_ok();
        assert!(
            ok && t.elapsed() < Duration::from_secs(5),
            "inside case {k}: {}",
            if ok { "too slow" } else { "panicked" }
        );
    }
}

/// A drawing read as the plug-in reads one at 300 pixels; if it is read, drawn as a test (it
/// must not panic, nor be past the limits it was read within), and drawn stretched to a strip
/// (as a part's picture may be: its buffers counted again there).
fn read_and_draw(b: &[u8]) {
    use resvg::tiny_skia::{Pixmap, Transform};
    if let Ok((tree, _)) = svg::parse_at(b, &Limits::default(), Some(300)) {
        let drawn = svg::test_draw(&tree, 300, 2_000);
        assert!(
            !matches!(drawn, svg::Drawn::Panicked | svg::Drawn::TooLarge),
            "the test drawing: {drawn:?}"
        );
        let size = tree.size();
        let mut strip = Pixmap::new(300, 30).expect("a pixmap");
        let stretch = Transform::from_scale(300.0 / size.width(), 30.0 / size.height());
        let _ = svg::render(&tree, stretch, &mut strip.as_mut());
    }
}

#[test]
fn drawings_never_panic_or_hang() {
    let seeds = vec![SVG.as_bytes().to_vec()];
    fuzz("svg", &seeds, Duration::from_secs(5), |b| {
        read_and_draw(b);
    });
}

#[test]
fn drawing_buffers_never_panic_or_hang() {
    let seeds = vec![
        include_bytes!("fixtures/vast-pattern/art/panel.svg").to_vec(),
        include_bytes!("fixtures/pattern-fuzz-17237.svg").to_vec(),
        include_bytes!("fixtures/embedded-png.svg").to_vec(),
        include_bytes!("fixtures/embedded-png-huge.svg").to_vec(),
        br"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>
        <defs><radialGradient id='g' gradientTransform='matrix(1 0 1 1 0 0)'>
        <stop offset='0' stop-color='#123'/><stop offset='1' stop-color='#abc'/>
        </radialGradient></defs><g transform='matrix(1 .1 .1 1 2 2)' opacity='.5'>
        <path d='M0 0 Q20 10 30 20 C40 30 50 10 90 20 Z' fill='url(#g)' stroke='#abc'
        stroke-width='2' stroke-dasharray='1 2' stroke-dashoffset='1'/></g></svg>"
            .to_vec(),
        br"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>
        <defs><pattern id='p' width='10' height='10' patternUnits='userSpaceOnUse'
        patternTransform='matrix(1 0 1 1 0 0)'><g opacity='.5'><rect width='10' height='10'/></g>
        </pattern><mask id='a'><rect width='100' height='100' fill='url(#p)'/></mask>
        <mask id='b' mask='url(#a)'><rect width='100' height='100'/></mask>
        <clipPath id='c'><rect width='100' height='100'/></clipPath>
        <clipPath id='d' clip-path='url(#c)'><rect width='100' height='100'/></clipPath>
        <filter id='f' filterUnits='userSpaceOnUse' width='100' height='100'>
        <feFlood result='f'/><feGaussianBlur stdDeviation='2'/><feOffset dx='1' dy='2'/>
        <feColorMatrix type='saturate' values='.5'/><feComposite in2='f' operator='in'/>
        <feMerge><feMergeNode in='SourceGraphic'/><feMergeNode/></feMerge></filter></defs>
        <g opacity='.5' mask='url(#b)' clip-path='url(#d)' filter='url(#f)'>
        <rect width='100' height='100' fill='url(#p)'/></g></svg>"
            .to_vec(),
    ];
    fuzz("buffers", &seeds, Duration::from_secs(5), |b| {
        read_and_draw(b);
    });
}

#[test]
fn manifests_never_panic() {
    let seeds = vec![MANIFEST.as_bytes().to_vec()];
    fuzz("manifest", &seeds, Duration::from_secs(2), |b| {
        let mut r = Report::default();
        let _ = manifest::read(b, &mut r);
        let _ = plugin_kit_skin_pack::licence::parse(&String::from_utf8_lossy(b));
    });
}

#[test]
fn fonts_never_panic_or_hang() {
    let font = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/urw-gothic/URWGothic-Book.otf"
    ))
    .expect("the built-in font");
    // Its head kept, so that mutations reach its tables rather than only its signature.
    fuzz("font", &[font], Duration::from_secs(5), |b| {
        let _ = font::read(b);
        let mut r = Report::default();
        let files = Files::default();
        font::check("fonts/f.otf", b, None, &files, &mut r);
    });
}
