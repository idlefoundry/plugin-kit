//! Writes a pack's folder as a pack file, as the marketplace delivers it (decisions.md
//! R-SKIN-PACKS): `cargo run -p skin-pack --example pack_archive -- <pack-folder>
//! <out.ifskin>`. Its files only (the system's litter left out), in order, the same bytes every
//! time: sign the folder first, then write it.

use std::path::Path;

use plugin_kit_skin_pack::{Files, Limits, Report, archive};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, out] = args.as_slice() else {
        eprintln!("pack_archive <pack-folder> <out.ifskin>");
        std::process::exit(2);
    };
    if !archive::is_pack_file(Path::new(out)) {
        eprintln!("the pack file's name ends in .{}", archive::EXTENSION);
        std::process::exit(2);
    }
    let mut report = Report::default();
    let files = Files::read_dir(Path::new(dir), &Limits::default(), &mut report);
    if report.rejected() {
        print!("{report}");
        eprintln!("not written: the folder holds what a pack may not");
        std::process::exit(1);
    }
    let bytes = archive::write(&files).expect("the pack file");
    std::fs::write(out, &bytes).expect("written");
    println!("{out}: {} files, {} bytes", files.len(), bytes.len());
}
