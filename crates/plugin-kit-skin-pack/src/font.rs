//! A pack's fonts: OpenType or TrueType, one face a file, read by the plug-ins' own font
//! parser; and a scan of what the font says of its licence.
//!
//! The scan sorts a font, it does not clear it: what a font's tables say was written by
//! whoever last edited the file. A font passes when its declared licence permits a pack to
//! carry it and its own tables agree; it is rejected when its tables forbid it (embedding
//! "restricted", or terms such as "personal use only"); everything else goes to review.

use crate::licence;
use crate::manifest::{Asset, Origin};
use crate::{Files, Report};

/// What a font says of itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    pub family: Option<String>,
    pub copyright: Option<String>,
    pub trademark: Option<String>,
    pub licence: Option<String>,
    pub licence_url: Option<String>,
    /// Its maker's four-letter code (OS/2's `achVendID`).
    pub vendor: Option<String>,
}

/// The licence a font's own tables name, if one known here.
fn named_licence(names: &Names) -> Option<&'static str> {
    let text = format!(
        "{} {}",
        names.licence.as_deref().unwrap_or(""),
        names.licence_url.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase();
    if text.contains("open font license")
        || text.contains("scripts.sil.org/ofl")
        || text.contains("openfontlicense.org")
    {
        Some("OFL-1.1")
    } else if text.contains("apache.org/licenses/license-2.0") || text.contains("apache license") {
        Some("Apache-2.0")
    } else if text.contains("creativecommons.org/publicdomain/zero") {
        Some("CC0-1.0")
    } else if text.contains("bitstream vera") {
        Some("Bitstream-Vera")
    } else {
        None
    }
}

/// Terms in a font's licence text that rule out a pack carrying it.
const FORBIDDING: [&str; 9] = [
    "personal use",
    "non-commercial",
    "noncommercial",
    "not for commercial",
    "demo",
    "trial version",
    "evaluation",
    "may not be redistributed",
    "not be embedded",
];

/// Reads a font's tables: its names, or why it cannot be read.
pub fn read(bytes: &[u8]) -> Result<(Names, Option<ttf_parser::Permissions>), String> {
    match bytes.get(..4) {
        Some(b"ttcf") => return Err("a font collection: one face a file".into()),
        Some(b"wOFF" | b"wOF2") => {
            return Err("a web font (WOFF): give the OpenType or TrueType font".into());
        }
        Some([0, 1, 0, 0] | b"OTTO" | b"true") => {}
        _ => return Err("not an OpenType or TrueType font".into()),
    }
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|e| format!("cannot be read: {e}"))?;
    if face.number_of_glyphs() == 0 {
        return Err("a font with no glyphs".into());
    }
    let mut names = Names::default();
    for n in face.names() {
        if !n.is_unicode() {
            continue;
        }
        let Some(s) = n.to_string() else { continue };
        let slot = match n.name_id {
            ttf_parser::name_id::COPYRIGHT_NOTICE => &mut names.copyright,
            ttf_parser::name_id::FAMILY => &mut names.family,
            ttf_parser::name_id::TRADEMARK => &mut names.trademark,
            ttf_parser::name_id::LICENSE => &mut names.licence,
            ttf_parser::name_id::LICENSE_URL => &mut names.licence_url,
            _ => continue,
        };
        slot.get_or_insert(s);
    }
    let raw = ttf_parser::RawFace::parse(bytes, 0).map_err(|e| format!("cannot be read: {e}"))?;
    if let Some(os2) = raw.table(ttf_parser::Tag::from_bytes(b"OS/2"))
        && let Some(v) = os2.get(58..62)
        && v.iter().all(|b| b.is_ascii_graphic() || *b == b' ')
    {
        names.vendor = Some(String::from_utf8_lossy(v).trim().to_owned());
    }
    Ok((names, face.permissions()))
}

/// Checks a font of a pack: its tables, and its licence as declared (`asset`) and as its own
/// tables give it; the pack's `files` for its licence's text.
pub fn check(path: &str, bytes: &[u8], asset: Option<&Asset>, files: &Files, report: &mut Report) {
    let p = Some(path);
    let (names, permissions) = match read(bytes) {
        Ok(r) => r,
        Err(why) => {
            report.reject("font.file", p, why);
            return;
        }
    };
    match permissions {
        Some(ttf_parser::Permissions::Restricted) => {
            report.reject(
                "font.embedding",
                p,
                "its embedding permission is \"restricted\": its maker forbids carrying it",
            );
            return;
        }
        Some(ttf_parser::Permissions::PreviewAndPrint) => report.review(
            "font.embedding",
            p,
            "its embedding permission is \"preview and print\" only",
        ),
        _ => {}
    }
    let own = names.licence.as_deref().unwrap_or("").to_ascii_lowercase();
    if let Some(term) = FORBIDDING.iter().find(|t| own.contains(*t)) {
        report.reject(
            "font.licence",
            p,
            format!("its own licence text says \"{term}\": a pack cannot carry it"),
        );
        return;
    }
    let named = named_licence(&names);
    let declared = asset.and_then(|a| a.license.as_deref());
    match declared {
        None => report.review(
            "font.licence",
            p,
            "no licence declared for it (its `[[assets]]` entry's `license`)",
        ),
        Some(d) if !licence::font_licence_ok(d) => report.review(
            "font.licence",
            p,
            format!(
                "under {d}: a review must confirm that its licence lets a pack carry it, and \
                 sell it so"
            ),
        ),
        Some(d) => match named {
            Some(n)
                if licence::parse(d).is_ok_and(|l| l.ids.iter().any(|id| id.starts_with(n))) =>
            {
                report.note(
                    "font.licence",
                    p,
                    format!("{d}, as declared and as its own tables name it"),
                );
            }
            Some(n) => report.review(
                "font.licence",
                p,
                format!("declared {d}, but its own tables name {n}"),
            ),
            None => report.review(
                "font.licence",
                p,
                format!("declared {d}, but its own tables name no licence known here"),
            ),
        },
    }
    // The OFL's and Apache's texts must travel with the font.
    let needs = match declared.or(named) {
        Some(d) if d.starts_with("OFL") => Some("SIL OPEN FONT LICENSE"),
        Some(d) if d.starts_with("Apache") => Some("Apache License"),
        _ => None,
    };
    if let Some(phrase) = needs {
        let carried = files.iter().any(|(f, b)| {
            crate::paths::kind(f) == Some(crate::paths::FileKind::Text)
                && std::str::from_utf8(b).is_ok_and(|t| {
                    t.to_ascii_lowercase()
                        .contains(&phrase.to_ascii_lowercase())
                })
        });
        if !carried {
            report.review(
                "font.notice",
                p,
                format!("its licence's text (\"{phrase}\") is not in the pack"),
            );
        }
    }
    let reserved = names
        .copyright
        .as_deref()
        .is_some_and(|c| c.to_ascii_lowercase().contains("reserved font name"))
        || declared.is_some_and(|d| d.contains("RFN"));
    if reserved && asset.is_some_and(|a| a.origin == Origin::Derived) {
        report.review(
            "font.reserved-name",
            p,
            "a changed font whose licence reserves its name: the changed one must not use it",
        );
    }
    if let Some(t) = &names.trademark {
        report.note(
            "font.trademark",
            p,
            format!("its name may be a trademark: \"{t}\""),
        );
    }
    if let Some(v) = &names.vendor {
        report.note("font.vendor", p, format!("made by vendor `{v}`"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_not_a_single_font_is_refused() {
        assert!(read(b"ttcf\0\0\0\0").unwrap_err().contains("collection"));
        assert!(read(b"wOF2....").unwrap_err().contains("web font"));
        assert!(read(b"<svg/>").is_err());
        assert!(read(b"OTTO").is_err());
    }

    #[test]
    fn a_licence_is_known_by_its_words() {
        let n = |l: &str, u: &str| Names {
            licence: Some(l.into()),
            licence_url: Some(u.into()),
            ..Names::default()
        };
        assert_eq!(
            named_licence(&n(
                "This Font Software is licensed under the SIL Open Font License, Version 1.1.",
                "https://openfontlicense.org"
            )),
            Some("OFL-1.1")
        );
        assert_eq!(
            named_licence(&n("", "http://www.apache.org/licenses/LICENSE-2.0")),
            Some("Apache-2.0")
        );
        assert_eq!(named_licence(&n("All rights reserved.", "")), None);
    }
}
