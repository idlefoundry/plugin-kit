//! A pack in one file (decisions.md R-SKIN-PACKS): a ZIP archive of its folder, named
//! `<name>.ifskin`, as a marketplace delivers it. It is read as a folder is, within the same
//! [`Limits`], and more strictly: each entry's size is never taken from its header (it is read
//! no further than its limit allows, whatever the header says), only stored and deflated entries
//! are read, and an encrypted entry, a link, or an archive of more entries than a pack may hold
//! is refused before anything is unpacked. Nothing is written to disk.

use std::io::{Cursor, Read};
use std::path::Path;

use crate::{Files, Limits, Report, paths};

/// A pack file's extension.
pub const EXTENSION: &str = "ifskin";

/// Whether `path` names a pack file.
pub fn is_pack_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some(EXTENSION)
}

/// The files of the pack file at `path` (the file at most the pack's whole size).
pub fn read_file(path: &Path, limits: &Limits, report: &mut Report) -> Files {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => {
            report.reject("pack.file", None, "the pack file is not a plain file");
            return Files::default();
        }
        Err(e) => {
            report.reject(
                "pack.file",
                None,
                format!("the pack file cannot be read: {e}"),
            );
            return Files::default();
        }
    }
    match crate::read_at_most(path, limits.total_bytes) {
        Ok(bytes) => read(&bytes, limits, report),
        Err(e) => {
            report.reject("pack.size", None, format!("the pack file: {e}"));
            Files::default()
        }
    }
}

/// The files of a pack file's bytes.
pub fn read(bytes: &[u8], limits: &Limits, report: &mut Report) -> Files {
    let mut zip = match zip::ZipArchive::new(Cursor::new(bytes)) {
        Ok(z) => z,
        Err(e) => {
            report.reject(
                "archive.format",
                None,
                format!("not a pack file (ZIP): {e}"),
            );
            return Files::default();
        }
    };
    // (A folder's entries count too: twice a pack's files, as a folder's walk allows.)
    if zip.len() > limits.files * 2 {
        report.reject(
            "pack.files",
            None,
            format!("{} entries, more than a pack may hold", zip.len()),
        );
        return Files::default();
    }
    // A pack's files at its top, or all in one folder at its top (as archivers make them).
    // (An archiver's own files, `__MACOSX/` beside the folder, say, left out first: they are
    // passed over, so they do not make a second folder at the top.)
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.name_for_index(i).map(str::to_owned))
        .filter(|n| !metadata(n))
        .collect();
    let prefix = common_folder(&names);
    let mut read: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = match zip.by_index(i) {
            Ok(e) => e,
            Err(e) => {
                report.reject(
                    "archive.entry",
                    None,
                    format!("an entry cannot be read: {e}"),
                );
                continue;
            }
        };
        let Ok(raw) = std::str::from_utf8(entry.name_raw()).map(str::to_owned) else {
            report.reject("path.unsafe", None, "an entry's name is not UTF-8");
            continue;
        };
        if entry.encrypted() {
            report.reject("archive.encrypted", Some(&raw), "an encrypted entry");
            continue;
        }
        if entry.is_symlink() {
            report.reject("path.link", Some(&raw), "a link: a pack holds no links");
            continue;
        }
        let name = prefix
            .as_deref()
            .and_then(|p| raw.strip_prefix(p))
            .unwrap_or(&raw)
            .to_owned();
        if metadata(&name) {
            continue;
        }
        if entry.is_dir() {
            let dir = name.trim_end_matches('/');
            if !dir.is_empty() {
                paths::check_dir(dir, limits, report);
            }
            continue;
        }
        let Some(kind) = paths::check(&name, limits, report) else {
            continue;
        };
        match entry.compression() {
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated => {}
            other => {
                report.reject(
                    "archive.method",
                    Some(&name),
                    format!("compressed as {other:?}: a pack file is stored or deflated"),
                );
                continue;
            }
        }
        let cap = match kind {
            paths::FileKind::Text => limits.text_bytes,
            paths::FileKind::Font => limits.font_bytes,
            _ => limits.file_bytes,
        };
        // Never more than its cap, whatever its header says.
        let mut bytes = Vec::new();
        if let Err(e) = (&mut entry).take(cap + 1).read_to_end(&mut bytes) {
            report.reject(
                "archive.entry",
                Some(&name),
                format!("cannot be unpacked: {e}"),
            );
            continue;
        }
        if bytes.len() as u64 > cap {
            report.reject(
                "file.size",
                Some(&name),
                format!("more than {cap} bytes unpacked"),
            );
            continue;
        }
        total += bytes.len() as u64;
        if total > limits.total_bytes {
            report.reject(
                "pack.size",
                None,
                format!("more than {} bytes unpacked", limits.total_bytes),
            );
            return Files::default();
        }
        if read.iter().any(|(n, _)| n == &name) {
            report.reject("path.duplicate", Some(&name), "the same name twice");
            continue;
        }
        read.push((name, bytes));
    }
    Files::from_memory(
        read.iter().map(|(n, b)| (n.as_str(), b.as_slice())),
        limits,
        report,
    )
}

/// An entry an archiver or a file manager adds (`__MACOSX/` and what is in it, `.DS_Store`...):
/// passed over.
fn metadata(name: &str) -> bool {
    let trimmed = name.trim_end_matches('/');
    paths::ignored(trimmed.rsplit('/').next().unwrap_or(trimmed))
        || trimmed.split('/').any(|p| p == "__MACOSX")
}

/// The one folder every entry is in, if there is one and no manifest at the top.
fn common_folder(names: &[String]) -> Option<String> {
    if names.iter().any(|n| n == crate::MANIFEST) {
        return None;
    }
    let first = names.first()?.split('/').next()?.to_owned();
    // Only a folder a pack could hold (never `..`, nothing absolute): what is stripped is then
    // never what made a name unsafe.
    let mut scratch = Report::default();
    if !paths::check_dir(&first, &Limits::default(), &mut scratch) {
        return None;
    }
    let prefix = format!("{first}/");
    names
        .iter()
        .all(|n| n.starts_with(&prefix) || n == &first)
        .then_some(prefix)
}

/// Writes the pack in `files` as a pack file: its entries in order, deflated, dated the same
/// every time (the same pack, the same bytes).
pub fn write(files: &Files) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut out = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644);
        for (path, bytes) in files.iter() {
            zip.start_file(path, options).map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(entries: &[(&str, &[u8], zip::CompressionMethod)]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut out);
            for (name, bytes, method) in entries {
                let o = zip::write::SimpleFileOptions::default().compression_method(*method);
                z.start_file(*name, o).expect("an entry");
                z.write_all(bytes).expect("written");
            }
            z.finish().expect("finished");
        }
        out.into_inner()
    }

    const DEFLATED: zip::CompressionMethod = zip::CompressionMethod::Deflated;

    #[test]
    fn a_pack_file_is_read_as_its_folder() {
        let z = zip_of(&[
            ("skin.toml", b"format = 1\n", DEFLATED),
            ("art/panel.svg", b"<svg/>", DEFLATED),
        ]);
        let mut r = Report::default();
        let files = read(&z, &Limits::default(), &mut r);
        assert!(r.findings.is_empty(), "{r}");
        assert_eq!(files.get("art/panel.svg"), Some(b"<svg/>".as_slice()));
        // The same in one folder at its top.
        let z = zip_of(&[
            ("night/skin.toml", b"format = 1\n", DEFLATED),
            ("night/LICENSE.txt", b"x", DEFLATED),
        ]);
        let files = read(&z, &Limits::default(), &mut r);
        assert!(
            files.contains("skin.toml") && files.contains("LICENSE.txt"),
            "{r}"
        );
        // The same with macOS's archiver's metadata beside its folder (passed over).
        let z = zip_of(&[
            ("night/skin.toml", b"format = 1\n", DEFLATED),
            ("night/LICENSE.txt", b"x", DEFLATED),
            ("__MACOSX/night/._skin.toml", b"x", DEFLATED),
            ("__MACOSX/._night", b"x", DEFLATED),
        ]);
        let mut r = Report::default();
        let files = read(&z, &Limits::default(), &mut r);
        assert!(r.findings.is_empty(), "{r}");
        assert!(files.contains("skin.toml") && !files.contains("__MACOSX/night/._skin.toml"));
        // Written and read again, the same.
        let again = read(&write(&files).expect("written"), &Limits::default(), &mut r);
        assert_eq!(again, files);
    }

    #[test]
    fn entries_that_could_leave_the_pack_are_refused() {
        for name in [
            "../escape.txt",
            "/etc/passwd.txt",
            "a\\b.txt",
            "C:/x.txt",
            ".hidden.txt",
            "../../skin.toml",
            "./skin.toml",
        ] {
            let z = zip_of(&[(name, b"x", DEFLATED)]);
            let mut r = Report::default();
            let files = read(&z, &Limits::default(), &mut r);
            assert!(files.is_empty() && r.rejected(), "{name}: {r}");
        }
    }

    /// A small entry that unpacks to far more than its limit: read no further than the limit.
    #[test]
    fn a_zip_bomb_is_stopped_at_its_limit() {
        let big = vec![b'a'; 3 << 20];
        let z = zip_of(&[("notes.txt", &big, DEFLATED)]);
        assert!(z.len() < 50_000, "{}", z.len());
        let mut r = Report::default();
        let files = read(&z, &Limits::default(), &mut r);
        assert!(!files.contains("notes.txt"));
        assert!(r.has("file.size"), "{r}");
    }

    #[test]
    fn too_many_entries_are_refused_before_any_is_read() {
        let entries: Vec<(String, Vec<u8>)> = (0..600)
            .map(|i| (format!("n{i}.txt"), vec![b'x']))
            .collect();
        let refs: Vec<(&str, &[u8], zip::CompressionMethod)> = entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice(), DEFLATED))
            .collect();
        let mut r = Report::default();
        let files = read(&zip_of(&refs), &Limits::default(), &mut r);
        assert!(files.is_empty() && r.has("pack.files"), "{r}");
    }

    /// A link inside a pack file, and two names that differ only in case: refused.
    #[test]
    fn links_and_names_differing_in_case_are_refused() {
        let mut out = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut out);
            let o = zip::write::SimpleFileOptions::default();
            z.add_symlink("notes.txt", "/etc/passwd", o)
                .expect("a link");
            z.start_file("README.md", o).expect("an entry");
            z.write_all(b"a").expect("written");
            z.start_file("readme.md", o).expect("an entry");
            z.write_all(b"b").expect("written");
            z.finish().expect("finished");
        }
        let mut r = Report::default();
        let files = read(&out.into_inner(), &Limits::default(), &mut r);
        assert!(!files.contains("notes.txt"));
        assert!(r.has("path.link") && r.has("path.duplicate"), "{r}");
    }

    #[test]
    fn what_is_not_a_zip_is_not_a_pack_file() {
        let mut r = Report::default();
        assert!(read(b"not a zip at all", &Limits::default(), &mut r).is_empty());
        assert!(r.has("archive.format"));
    }
}
