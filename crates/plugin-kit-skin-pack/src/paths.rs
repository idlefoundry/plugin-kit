//! A pack's paths and its files' kinds: what a pack may hold, by name and by content.

use crate::{Limits, Report};

/// What a file of a pack is, by its extension: nothing else may be in a pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// The manifest, a layout, a licence, notices, a read-me (`.toml`, `.txt`, `.md`).
    Text,
    Png,
    Svg,
    /// OpenType or TrueType (`.otf`, `.ttf`).
    Font,
}

/// A path's kind, by its extension (lower case only: one name, one meaning on every system).
pub fn kind(path: &str) -> Option<FileKind> {
    let ext = path.rsplit_once('.').map(|(_, e)| e)?;
    Some(match ext {
        "toml" | "txt" | "md" => FileKind::Text,
        "png" => FileKind::Png,
        "svg" => FileKind::Svg,
        "otf" | "ttf" => FileKind::Font,
        _ => return None,
    })
}

/// Names a system leaves in a folder, passed over without reading them.
pub fn ignored(name: &str) -> bool {
    matches!(name, ".DS_Store" | "Thumbs.db" | "desktop.ini" | "__MACOSX")
}

/// Names Windows keeps for devices, whatever their extension.
const DEVICES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Whether one part of a path (a folder's or a file's name) is safe on every system: ASCII
/// letters, digits, `-`, `_` and `.`, not starting with a dot, not ending with one, not a
/// device's name.
fn safe_part(part: &str) -> Result<(), &'static str> {
    if part.is_empty() {
        return Err("an empty part");
    }
    if part == "." || part == ".." {
        return Err("a relative part (`.` or `..`)");
    }
    if part.starts_with('.') {
        return Err("a hidden name");
    }
    if part.ends_with('.') {
        return Err("a name ending in a dot");
    }
    if !part
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("a character other than letters, digits, `-`, `_` and `.`");
    }
    let stem = part.split('.').next().unwrap_or(part).to_ascii_lowercase();
    if DEVICES.contains(&stem.as_str()) {
        return Err("a name Windows keeps for a device");
    }
    Ok(())
}

/// Checks a path in a pack (`/` between its parts, relative, within `limits`): its kind, or
/// `None` (reported) if a pack may not hold it.
pub fn check(path: &str, limits: &Limits, report: &mut Report) -> Option<FileKind> {
    if path.len() > limits.path_bytes {
        report.reject(
            "path.length",
            Some(path),
            format!("longer than {} bytes", limits.path_bytes),
        );
        return None;
    }
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        report.reject(
            "path.unsafe",
            Some(path),
            "an absolute path, or one with `\\` or `:`",
        );
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > limits.depth {
        report.reject(
            "path.depth",
            Some(path),
            format!("deeper than {} folders", limits.depth - 1),
        );
        return None;
    }
    for part in &parts {
        if let Err(why) = safe_part(part) {
            report.reject("path.unsafe", Some(path), why);
            return None;
        }
    }
    let k = kind(path);
    if k.is_none() {
        report.reject(
            "file.type",
            Some(path),
            "a file a pack may not hold (only .toml, .txt, .md, .png, .svg, .otf and .ttf)",
        );
    }
    k
}

/// Checks a folder's path in a pack: whether to read it.
pub fn check_dir(path: &str, limits: &Limits, report: &mut Report) -> bool {
    if path.len() > limits.path_bytes {
        report.reject(
            "path.length",
            Some(path),
            format!("longer than {} bytes", limits.path_bytes),
        );
        return false;
    }
    for part in path.split('/') {
        if let Err(why) = safe_part(part) {
            report.reject("path.unsafe", Some(path), why);
            return false;
        }
    }
    true
}

/// What a file is by its first bytes, if it is code or an archive (which a pack never holds,
/// whatever its name).
pub fn executable(bytes: &[u8]) -> Option<&'static str> {
    let starts = |m: &[u8]| bytes.starts_with(m);
    if starts(&[0xfe, 0xed, 0xfa, 0xce])
        || starts(&[0xfe, 0xed, 0xfa, 0xcf])
        || starts(&[0xce, 0xfa, 0xed, 0xfe])
        || starts(&[0xcf, 0xfa, 0xed, 0xfe])
    {
        return Some("a Mach-O program or library");
    }
    if starts(&[0xca, 0xfe, 0xba, 0xbe]) {
        return Some("a universal program or a Java class");
    }
    if starts(b"\x7fELF") {
        return Some("an ELF program or library");
    }
    if starts(b"MZ") {
        return Some("a Windows program or library");
    }
    if starts(b"\0asm") {
        return Some("WebAssembly");
    }
    if starts(b"#!") {
        return Some("a script");
    }
    if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        return Some("a ZIP archive");
    }
    if starts(&[0x1f, 0x8b]) || starts(b"7z\xbc\xaf\x27\x1c") || starts(b"Rar!") {
        return Some("a compressed archive");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(path: &str) -> bool {
        let mut r = Report::default();
        check(path, &Limits::default(), &mut r).is_none() && r.rejected()
    }

    #[test]
    fn paths_that_could_leave_the_pack_are_rejected() {
        for p in [
            "../escape.png",
            "pictures/../../escape.png",
            "/etc/passwd.txt",
            "C:/Windows/x.png",
            "pictures\\knob.png",
            "./knob.png",
            "pictures//knob.png",
            "",
        ] {
            assert!(rejected(p), "{p}");
        }
    }

    #[test]
    fn names_unsafe_on_some_system_are_rejected() {
        for p in [
            ".hidden.png",
            "knob.png.",
            "con.png",
            "pictures/LPT1.txt",
            "kn ob.png",
            "knöb.png",
            "knob?.png",
            "a/b/c/d/e.png",
        ] {
            assert!(rejected(p), "{p}");
        }
    }

    #[test]
    fn only_the_formats_a_pack_may_hold_are_read() {
        for p in [
            "x.dylib", "x.exe", "x.js", "x.lua", "x.wasm", "x.zip", "x.PNG", "x.html", "x.json",
            "x",
        ] {
            assert!(rejected(p), "{p}");
        }
        let mut r = Report::default();
        for p in [
            "skin.toml",
            "LICENSE.txt",
            "pictures/knob.png",
            "art/face.svg",
            "fonts/Sans-Bold.otf",
            "README.md",
        ] {
            assert!(check(p, &Limits::default(), &mut r).is_some(), "{p}");
        }
        assert!(r.findings.is_empty());
    }

    #[test]
    fn code_is_known_by_its_first_bytes_whatever_its_name() {
        assert!(executable(b"\xcf\xfa\xed\xfe....").is_some());
        assert!(executable(b"\x7fELF....").is_some());
        assert!(executable(b"MZ\x90\0").is_some());
        assert!(executable(b"#!/bin/sh\nrm -rf ~").is_some());
        assert!(executable(b"PK\x03\x04").is_some());
        assert!(executable(b"\x89PNG\r\n\x1a\n").is_none());
        assert!(executable(b"format = 1\n").is_none());
    }
}
