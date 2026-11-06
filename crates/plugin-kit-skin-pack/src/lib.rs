//! Idle Foundry's skin packs, every plug-in's (plugin-kit's K-SKIN-PACK; first made for the CA-72,
//! whose `docs/decisions.md` the `R-` references here are, R-SKIN-PACKS the format): a folder of artwork and declarative
//! appearance data that a plug-in's open-source renderer draws its editor with. A pack holds no
//! code: its files are a manifest (`skin.toml`), the plug-in's layout data, pictures (PNG),
//! drawings (a strict subset of SVG), fonts (OpenType or TrueType) and texts (its licence, its
//! notices, a read-me). The format is documented in the CA-72's `docs/skin-packs.md`.
//!
//! Everything here treats a pack as untrusted input. A folder is read only within [`Limits`]:
//! no link is followed, no path leaves the folder, no file of a type a pack may not hold is
//! read, and no file or picture past its size. Each file is checked by what it is, not by what
//! its name says. What the checks find is a [`Report`]: what rejects the pack (it is not
//! loaded), what a marketplace's review must look at (it loads, for its owner, but is not
//! cleared for sale), and notes.
//!
//! Price is no part of a pack: it is the store's. A pack's licence, its creators, its assets'
//! provenance and its third-party notices are each named in its manifest, so that every pack's
//! terms stay its own: the plug-in's licence is not applied to a pack, nor a pack's to the
//! plug-in. A seller's declarations are kept as declared, never as cleared.

pub mod archive;
pub mod font;
pub mod keys;
pub mod licence;
pub mod manifest;
pub mod paths;
pub mod picture;
pub mod signature;
pub mod svg;

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

pub use manifest::Manifest;

/// The manifest's name in a pack.
pub const MANIFEST: &str = "skin.toml";

/// The format this crate reads.
pub const FORMAT: u32 = 1;

/// How much a pack may hold and cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Files in the pack, and directories deep.
    pub files: usize,
    pub depth: usize,
    /// A path's length in bytes.
    pub path_bytes: usize,
    /// A file's size, and all files' together.
    pub file_bytes: u64,
    pub total_bytes: u64,
    /// A text's (the manifest's, a layout's, a licence's).
    pub text_bytes: u64,
    /// A picture's width or height, and all pictures' pixels together, decoded.
    pub picture_side: u32,
    pub picture_pixels: u64,
    /// An SVG drawing's elements (`use` counted as what it repeats), its paths' segments,
    /// filter primitives and the largest blur's deviation (in its own units).
    pub svg_nodes: usize,
    pub svg_segments: usize,
    pub svg_filters: usize,
    pub svg_blur: f32,
    /// A pattern's tiles over what it fills, all its uses together.
    pub svg_tiles: usize,
    /// What a drawing may cost to draw, in pixel-equivalents at the test drawing's size
    /// (`svg::Cost::work`): about 100 ms on an M4 Pro, half a second on a machine five times
    /// slower. It bounds what drawing costs; the timed test is a backstop.
    pub svg_work: f64,
    /// What drawing it may allocate, at the test drawing's size: pixels in any one of its
    /// buffers (a layer, a pattern's tile, a filter's region, a clip's or a mask's copy), and
    /// bytes in all of them together, as if none were freed (`svg::Cost::buffer_bytes`).
    /// Checked before anything is drawn: resvg aborts the process when an allocation fails.
    pub svg_buffer_pixels: u64,
    pub svg_buffer_bytes: u64,
    /// A drawing's test, drawn with its longer side this many pixels: its time limit. (A
    /// backstop well past what `svg_work` allows on a slow machine, so that a drawing within
    /// the cost is never refused for the machine's speed or how busy it is: at 250 ms, the
    /// starter's own drawing was refused on CI's runners and under a busy desktop.)
    pub svg_test_side: u32,
    pub svg_test_ms: u64,
    /// Fonts in a pack, and a font's size.
    pub fonts: usize,
    pub font_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            files: 256,
            depth: 4,
            path_bytes: 128,
            file_bytes: 8 << 20,
            total_bytes: 64 << 20,
            text_bytes: 256 << 10,
            picture_side: 4096,
            // About three times the official REALISTIC look's pictures (decisions.md R-SKIN-PACKS:
            // 48 million let one look hold some 400 MB).
            picture_pixels: 16_000_000,
            svg_nodes: 20_000,
            svg_segments: 200_000,
            svg_filters: 32,
            svg_blur: 64.0,
            svg_tiles: 4096,
            svg_work: 60.0e6,
            // (As the pictures' pixels: a drawing 3000 pixels square is 9 million.)
            svg_buffer_pixels: 16_000_000,
            svg_buffer_bytes: 256 << 20,
            svg_test_side: 3000,
            svg_test_ms: 2000,
            fonts: 4,
            font_bytes: 4 << 20,
        }
    }
}

/// What a finding means for the pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The pack is not loaded.
    Reject,
    /// The pack loads for its owner, but a marketplace's review must look at this before it is
    /// listed: it is never cleared by the checks.
    Review,
    /// For the record.
    Note,
}

/// One thing the checks found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// A stable name for what was found (`path.unsafe`, `svg.external`, `font.licence`, ...).
    pub code: &'static str,
    /// The file it is about, in the pack.
    pub path: Option<String>,
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.severity {
            Severity::Reject => "rejected",
            Severity::Review => "review",
            Severity::Note => "note",
        };
        match &self.path {
            Some(p) => write!(f, "{what} [{}] {p}: {}", self.code, self.message),
            None => write!(f, "{what} [{}] {}", self.code, self.message),
        }
    }
}

/// What the checks of a pack found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn add(
        &mut self,
        severity: Severity,
        code: &'static str,
        path: Option<&str>,
        message: impl Into<String>,
    ) {
        self.findings.push(Finding {
            severity,
            code,
            path: path.map(str::to_owned),
            message: message.into(),
        });
    }

    pub fn reject(&mut self, code: &'static str, path: Option<&str>, message: impl Into<String>) {
        self.add(Severity::Reject, code, path, message);
    }

    pub fn review(&mut self, code: &'static str, path: Option<&str>, message: impl Into<String>) {
        self.add(Severity::Review, code, path, message);
    }

    pub fn note(&mut self, code: &'static str, path: Option<&str>, message: impl Into<String>) {
        self.add(Severity::Note, code, path, message);
    }

    /// The pack is not to be loaded.
    pub fn rejected(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Reject)
    }

    /// A review must look at the pack before it is listed.
    pub fn needs_review(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Review)
    }

    /// Whether a finding with `code` is in the report.
    pub fn has(&self, code: &str) -> bool {
        self.findings.iter().any(|f| f.code == code)
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sorted: Vec<&Finding> = self.findings.iter().collect();
        sorted.sort_by_key(|f| f.severity);
        for finding in sorted {
            writeln!(f, "{finding}")?;
        }
        Ok(())
    }
}

/// A pack's files, by their path in it (`/` between its parts), as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Files {
    entries: BTreeMap<String, Vec<u8>>,
}

impl Files {
    /// Files given in memory (a pack built into a plug-in, a test's). Their paths are checked
    /// as a folder's are.
    pub fn from_memory<'a>(
        files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
        limits: &Limits,
        report: &mut Report,
    ) -> Self {
        let mut out = Self::default();
        let mut total = 0u64;
        for (path, bytes) in files {
            if out.entries.len() >= limits.files {
                report.reject(
                    "pack.files",
                    None,
                    format!("more than {} files", limits.files),
                );
                break;
            }
            if paths::check(path, limits, report).is_none() {
                continue;
            }
            total += bytes.len() as u64;
            if !out.admit(path, bytes.len() as u64, total, limits, report) {
                continue;
            }
            out.entries.insert(path.to_owned(), bytes.to_vec());
        }
        out
    }

    /// The files of the folder at `root`, read within `limits`: a link (to a file or a folder)
    /// is never followed, nor anything but a plain file read; a path a pack may not hold, or a
    /// file past its size, is reported and not read. The folder's own name is not checked: it
    /// is the user's.
    pub fn read_dir(root: &Path, limits: &Limits, report: &mut Report) -> Self {
        let mut out = Self::default();
        match std::fs::symlink_metadata(root) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                report.reject("pack.folder", None, "the pack is not a folder");
                return out;
            }
            Err(e) => {
                report.reject("pack.folder", None, format!("the pack cannot be read: {e}"));
                return out;
            }
        }
        let mut total = 0u64;
        let mut seen = 0usize;
        let mut stack = vec![(root.to_path_buf(), String::new(), 0usize)];
        while let Some((dir, prefix, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                report.reject("pack.folder", Some(&prefix), "a folder cannot be read");
                continue;
            };
            let mut names: Vec<_> = entries.filter_map(Result::ok).collect();
            names.sort_by_key(std::fs::DirEntry::file_name);
            for entry in names {
                seen += 1;
                if seen > limits.files * 2 {
                    report.reject(
                        "pack.files",
                        None,
                        format!("more than {} files", limits.files),
                    );
                    return out;
                }
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    report.reject(
                        "path.unsafe",
                        Some(&prefix),
                        "a name in the pack is not Unicode",
                    );
                    continue;
                };
                if paths::ignored(name) {
                    continue;
                }
                let path = if prefix.is_empty() {
                    name.to_owned()
                } else {
                    format!("{prefix}/{name}")
                };
                // Never through a link: its own metadata, not its target's.
                let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                    report.reject("pack.folder", Some(&path), "cannot be read");
                    continue;
                };
                let kind = meta.file_type();
                if kind.is_symlink() {
                    report.reject("path.link", Some(&path), "a link: a pack holds no links");
                    continue;
                }
                if kind.is_dir() {
                    if depth + 1 >= limits.depth {
                        report.reject(
                            "path.depth",
                            Some(&path),
                            format!("deeper than {} folders", limits.depth - 1),
                        );
                        continue;
                    }
                    if paths::check_dir(&path, limits, report) {
                        stack.push((entry.path(), path, depth + 1));
                    }
                    continue;
                }
                if !kind.is_file() {
                    report.reject("path.special", Some(&path), "not a plain file");
                    continue;
                }
                if out.entries.len() >= limits.files {
                    report.reject(
                        "pack.files",
                        None,
                        format!("more than {} files", limits.files),
                    );
                    return out;
                }
                if paths::check(&path, limits, report).is_none() {
                    continue;
                }
                total += meta.len();
                if !out.admit(&path, meta.len(), total, limits, report) {
                    continue;
                }
                match read_at_most(&entry.path(), limits.file_bytes) {
                    Ok(bytes) => {
                        out.entries.insert(path, bytes);
                    }
                    Err(e) => report.reject("pack.folder", Some(&path), e),
                }
            }
        }
        out
    }

    /// Whether a file of `len` bytes (all files' together `total`) may be read.
    fn admit(
        &self,
        path: &str,
        len: u64,
        total: u64,
        limits: &Limits,
        report: &mut Report,
    ) -> bool {
        let lower = path.to_ascii_lowercase();
        if self.entries.keys().any(|k| k.to_ascii_lowercase() == lower) {
            report.reject(
                "path.duplicate",
                Some(path),
                "another file's name differs from it only in case",
            );
            return false;
        }
        let cap = match paths::kind(path) {
            Some(paths::FileKind::Text) => limits.text_bytes,
            Some(paths::FileKind::Font) => limits.font_bytes,
            _ => limits.file_bytes,
        };
        if len > cap {
            report.reject(
                "file.size",
                Some(path),
                format!("{len} bytes, more than {cap}"),
            );
            return false;
        }
        if total > limits.total_bytes {
            report.reject(
                "pack.size",
                Some(path),
                format!(
                    "the pack's files are more than {} bytes",
                    limits.total_bytes
                ),
            );
            return false;
        }
        true
    }

    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.entries.get(path).map(Vec::as_slice)
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(path)
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// At most `limit` bytes of a file: more is an error (the file grew since its size was read).
pub(crate) fn read_at_most(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("cannot be read: {e}"))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot be read: {e}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("more than {limit} bytes"));
    }
    Ok(bytes)
}

/// What the plug-in checking a pack brings to the checks.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// The plug-in's id (`ca-72`): a pack for another is not loaded.
    pub plugin: &'a str,
    /// The plug-in's own artwork, under its licence (`official_licence`): a pack under another
    /// licence holding a copy of one is put to review (it needs the plug-in's makers'
    /// additional permission).
    pub official: &'a [(&'a str, &'a [u8])],
    pub official_licence: &'a str,
    /// The pack is one of the plug-in's makers' own, built in (its id may use their prefix).
    pub built_in: bool,
    /// Draw each SVG drawing as a test, against its time limit (a pack being installed or
    /// reviewed; a built-in pack's are tested by the plug-in's tests instead).
    pub test_drawings: bool,
    /// Idle Foundry's and any future marketplace's public keys (`signature`).
    pub keys: &'a [signature::TrustedKey],
    /// The plug-in's version (`major.minor.patch`), against which a pack's `requires` is
    /// checked; `None` where no plug-in is (a marketplace's tools).
    pub plugin_version: Option<&'a str>,
}

/// The id prefix kept for the plug-ins' makers' own packs.
pub const OFFICIAL_PREFIX: &str = "com.idlefoundry.";

/// A pack that its checks did not reject.
#[derive(Clone, Debug)]
pub struct Pack {
    pub manifest: Manifest,
    pub files: Files,
    /// What the checks found (no rejection).
    pub report: Report,
    /// Its signature, checked: who published it, if signed by a trusted key.
    pub signed: Option<signature::Signed>,
}

impl Pack {
    /// The pack at `path`: a folder, or a pack file (`<name>.ifskin`).
    pub fn open(path: &Path, limits: &Limits, context: &Context<'_>) -> Result<Pack, Report> {
        let mut report = Report::default();
        let files = if archive::is_pack_file(path) {
            archive::read_file(path, limits, &mut report)
        } else {
            Files::read_dir(path, limits, &mut report)
        };
        Self::check(files, report, limits, context)
    }

    /// The pack of `files`, its reading's findings in `report`, checked.
    pub fn check(
        files: Files,
        mut report: Report,
        limits: &Limits,
        context: &Context<'_>,
    ) -> Result<Pack, Report> {
        let Some(text) = files.get(MANIFEST) else {
            report.reject(
                "manifest.missing",
                None,
                format!("no {MANIFEST} at the pack's top"),
            );
            return Err(report);
        };
        let Some(manifest) = manifest::read(text, &mut report) else {
            return Err(report);
        };
        let signed = signature::verify(
            &files,
            (&manifest.id, &manifest.version),
            context.keys,
            &mut report,
        );
        let first_party = signed
            .as_ref()
            .is_some_and(|s| s.publisher == signature::Publisher::FirstParty);
        manifest::check(&manifest, &files, context, first_party, &mut report);
        check_files(&files, &manifest, limits, context, &mut report);
        if report.rejected() {
            Err(report)
        } else {
            Ok(Pack {
                manifest,
                files,
                report,
                signed,
            })
        }
    }

    /// A text file of the pack (UTF-8, checked as it was read).
    pub fn text(&self, path: &str) -> Option<&str> {
        self.files
            .get(path)
            .and_then(|b| std::str::from_utf8(b).ok())
    }
}

/// Each file checked by what it is.
fn check_files(
    files: &Files,
    manifest: &Manifest,
    limits: &Limits,
    context: &Context<'_>,
    report: &mut Report,
) {
    let mut pixels = 0u64;
    let mut fonts = 0usize;
    for (path, bytes) in files.iter() {
        if let Some(what) = paths::executable(bytes) {
            report.reject(
                "file.executable",
                Some(path),
                format!("{what}: a pack holds no code"),
            );
            continue;
        }
        match paths::kind(path) {
            Some(paths::FileKind::Text) => {
                if std::str::from_utf8(bytes).is_err() {
                    report.reject("text.encoding", Some(path), "not UTF-8 text");
                }
            }
            Some(paths::FileKind::Png) => {
                if let Some((w, h)) = picture::check(path, bytes, limits, report) {
                    pixels += u64::from(w) * u64::from(h);
                }
            }
            Some(paths::FileKind::Svg) => {
                pixels += svg::check(path, bytes, limits, context.test_drawings, report);
            }
            Some(paths::FileKind::Font) => {
                fonts += 1;
                font::check(path, bytes, manifest.asset(path), files, report);
            }
            // (`paths::check` reported it.)
            None => {}
        }
        // A copy of the plug-in's own artwork, in a pack under another licence.
        if let Some((name, _)) = context
            .official
            .iter()
            .find(|(_, official)| *official == bytes)
        {
            let licence = manifest
                .asset(path)
                .and_then(|a| a.license.as_deref())
                .unwrap_or(&manifest.license);
            if licence::same_terms(licence, context.official_licence) {
                report.note(
                    "asset.official",
                    Some(path),
                    format!("the plug-in's own {name}, under its licence ({licence})"),
                );
            } else {
                report.review(
                    "asset.official-copy",
                    Some(path),
                    format!(
                        "a copy of the plug-in's own {name} ({}) in a pack under {licence}: it \
                         needs the plug-in's makers' additional permission",
                        context.official_licence
                    ),
                );
            }
        }
    }
    if pixels > limits.picture_pixels {
        report.reject(
            "pack.pixels",
            None,
            format!(
                "the pictures are {pixels} pixels together, more than {}",
                limits.picture_pixels
            ),
        );
    }
    if fonts > limits.fonts {
        report.reject(
            "pack.fonts",
            None,
            format!("{fonts} fonts, more than {}", limits.fonts),
        );
    }
}

/// A file's SHA-256, in lower-case hexadecimal (an asset's in its manifest).
pub fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
