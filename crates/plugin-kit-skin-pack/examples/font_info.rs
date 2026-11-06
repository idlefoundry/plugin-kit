//! What a font says of itself, as a pack's check reads it (decisions.md R-SKIN-PACKS):
//! `cargo run -p skin-pack --example font_info -- <font.otf|.ttf> [text]`. Its names (family,
//! copyright, licence and its link, trademark), its maker's code, its embedding permission, which
//! printable ASCII characters it lacks, and whether it has a glyph for each character of `text`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("font_info <font> [text]");
        std::process::exit(2);
    };
    let bytes = std::fs::read(path).expect("the font");
    let (names, permissions) = match plugin_kit_skin_pack::font::read(&bytes) {
        Ok(r) => r,
        Err(e) => {
            println!("cannot be read: {e}");
            std::process::exit(1);
        }
    };
    println!("{names:#?}\nembedding: {permissions:?}");
    let face = ttf_parser::Face::parse(&bytes, 0).expect("a face");
    let missing: String = (0x20u8..0x7f)
        .map(char::from)
        .filter(|c| face.glyph_index(*c).is_none())
        .collect();
    println!("printable ASCII it lacks: {missing:?}");
    if let Some(text) = args.get(1) {
        for c in text.chars().filter(|c| !c.is_whitespace()) {
            println!(
                "  {c} U+{:04X}: {}",
                c as u32,
                face.glyph_index(c).is_some()
            );
        }
    }
}
