//! A pack's manifest, `skin.toml`: what the pack is, its licence, its creators, each asset's
//! provenance, its third-party notices and its seller's declarations; and the plug-in's own
//! table, `[look]`, which only the plug-in reads.

use serde::Deserialize;

use crate::licence::{self, Terms};
use crate::paths::{self, FileKind};
use crate::{Context, FORMAT, Files, OFFICIAL_PREFIX, Report};

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The format's version ([`FORMAT`]).
    pub format: u32,
    /// Lower case, dotted, its creator's own (`org.example.night-panel`).
    pub id: String,
    pub name: String,
    /// `major.minor.patch`.
    pub version: String,
    /// The plug-in it is for (`ca-72`).
    pub plugin: String,
    /// The plug-in's version it needs at least (`ca-72 >= 0.4.0`), when it uses something an
    /// earlier one does not have: an earlier one then says so, rather than drawing it wrong.
    #[serde(default)]
    pub requires: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    /// The pack's licence: an SPDX expression or a `LicenseRef-`.
    pub license: String,
    /// Its licence's texts in the pack.
    #[serde(default)]
    pub license_files: Vec<String>,
    #[serde(default)]
    pub creators: Vec<Creator>,
    #[serde(default)]
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub notices: Vec<Notice>,
    #[serde(default)]
    pub declarations: Option<Declarations>,
    /// The plug-in's table: its layout, pictures, drawings and fonts.
    #[serde(default)]
    pub look: Option<toml::Value>,
}

/// One who made the pack.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Creator {
    pub name: String,
    /// What they did (`artwork`, `layout`, ...).
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// Where an asset came from.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// Made by the pack's creators.
    Original,
    /// Made with a generator (its `tool` named).
    Generated,
    /// Made from another work (its `source` and its `license` named).
    Derived,
    /// Another's work, included (its `source` and its `license` named).
    ThirdParty,
}

/// An asset's provenance.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub path: String,
    pub origin: Origin,
    #[serde(default)]
    pub author: Option<String>,
    /// The tool it was made with (a generator's name and version).
    #[serde(default)]
    pub tool: Option<String>,
    /// The work it came from (a URL or a citation).
    #[serde(default)]
    pub source: Option<String>,
    /// Its licence, if not the pack's.
    #[serde(default)]
    pub license: Option<String>,
    /// Its SHA-256, checked if given.
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// A third-party work's notice, as its licence asks.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub name: String,
    pub license: String,
    pub file: String,
}

/// What a seller declares, as a marketplace's review needs it: that they may distribute every
/// asset under the pack's licence, and that they authorize the marketplace to license the pack
/// to its customers. Declared, never cleared: a review decides.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Declarations {
    pub rights_to_distribute: bool,
    pub authorizes_customer_licensing: bool,
    /// The seller agreement's version they declared under.
    #[serde(default)]
    pub agreement: Option<String>,
    pub declared_by: String,
    /// `YYYY-MM-DD`.
    pub declared_on: String,
}

impl Manifest {
    /// An asset's provenance, by its path.
    pub fn asset(&self, path: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.path == path)
    }

    /// The pack's licence's terms.
    pub fn terms(&self) -> Option<Terms> {
        licence::parse(&self.license).ok().map(|l| l.terms)
    }
}

/// Keys a store's listing has and a pack never does.
const STORE_KEYS: [&str; 5] = ["price", "currency", "sku", "license_key", "drm"];

/// The manifest read from its text, or `None` (reported).
pub fn read(text: &[u8], report: &mut Report) -> Option<Manifest> {
    let Ok(text) = std::str::from_utf8(text) else {
        report.reject("manifest.encoding", Some(crate::MANIFEST), "not UTF-8 text");
        return None;
    };
    let table: toml::Table = match toml::from_str(text) {
        Ok(t) => t,
        Err(e) => {
            report.reject("manifest.syntax", Some(crate::MANIFEST), e.to_string());
            return None;
        }
    };
    for key in STORE_KEYS {
        if table.contains_key(key) {
            report.reject(
                "manifest.store",
                Some(crate::MANIFEST),
                format!(
                    "`{key}`: a price and its terms of sale are the store's, never a pack's (its \
                     licence is independent of them)"
                ),
            );
            return None;
        }
    }
    match table.get("format").and_then(toml::Value::as_integer) {
        Some(f) if f == i64::from(FORMAT) => {}
        Some(f) if f > i64::from(FORMAT) => {
            report.reject(
                "manifest.format",
                Some(crate::MANIFEST),
                format!("format {f}: a newer plug-in is needed (this one reads {FORMAT})"),
            );
            return None;
        }
        _ => {
            report.reject(
                "manifest.format",
                Some(crate::MANIFEST),
                format!("no `format = {FORMAT}`"),
            );
            return None;
        }
    }
    match toml::Value::Table(table).try_into::<Manifest>() {
        Ok(m) => Some(m),
        Err(e) => {
            report.reject("manifest.fields", Some(crate::MANIFEST), e.to_string());
            None
        }
    }
}

/// Text a person reads: not empty, not too long, no control characters.
fn plain(s: &str, most: usize) -> bool {
    !s.trim().is_empty() && s.chars().count() <= most && !s.chars().any(char::is_control)
}

fn valid_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    id.len() <= 64
        && parts.len() >= 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn valid_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether version `a` (`major.minor.patch`) is newer than `b`.
fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    parts(a) > parts(b)
}

fn valid_date(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Checks the manifest against the pack's files and the plug-in (`first_party`: the pack is
/// signed as Idle Foundry's own, and may use its id prefix).
pub fn check(
    m: &Manifest,
    files: &Files,
    context: &Context<'_>,
    first_party: bool,
    report: &mut Report,
) {
    let at = Some(crate::MANIFEST);
    if !valid_id(&m.id) {
        report.reject(
            "manifest.id",
            at,
            format!(
                "`{}`: an id is lower-case letters, digits and `-`, in two or more parts \
                 between dots (`org.example.night-panel`)",
                m.id
            ),
        );
    } else if m.id.starts_with(OFFICIAL_PREFIX) && !context.built_in && !first_party {
        report.reject(
            "manifest.id",
            at,
            format!(
                "`{OFFICIAL_PREFIX}` is kept for the plug-in's makers' own packs (built in, or \
                 signed as theirs)"
            ),
        );
    }
    if !plain(&m.name, 64) {
        report.reject("manifest.name", at, "a name of 1 to 64 characters");
    }
    if let Some(s) = &m.summary
        && !plain(s, 280)
    {
        report.reject("manifest.summary", at, "a summary of 1 to 280 characters");
    }
    if !valid_version(&m.version) {
        report.reject("manifest.version", at, "a version is `major.minor.patch`");
    }
    if let Some(req) = &m.requires {
        let parsed = req
            .split_once(">=")
            .map(|(p, v)| (p.trim(), v.trim()))
            .filter(|(p, v)| *p == m.plugin && valid_version(v));
        match (parsed, context.plugin_version) {
            (None, _) => report.reject(
                "manifest.requires",
                at,
                format!(
                    "`{req}`: `requires` is \"{} >= major.minor.patch\"",
                    m.plugin
                ),
            ),
            (Some((_, needed)), Some(have)) if newer(needed, have) => report.reject(
                "manifest.requires",
                at,
                format!(
                    "it needs {} {needed} or newer: this is {have}",
                    m.plugin.to_uppercase()
                ),
            ),
            _ => {}
        }
    }
    if m.plugin != context.plugin {
        report.reject(
            "manifest.plugin",
            at,
            format!("for `{}`, not `{}`", m.plugin, context.plugin),
        );
    }

    // The licence, and its texts.
    let mut text_files: Vec<&str> = Vec::new();
    match licence::parse(&m.license) {
        Ok(l) => {
            let words = match l.terms {
                Terms::Open => "open",
                Terms::Restricted => "public with restrictions",
                Terms::Proprietary => "the creator's own terms",
            };
            report.note(
                "licence.terms",
                at,
                format!(
                    "the pack's own licence, {} ({words}); the plug-in's licence does not apply \
                     to it, nor its to the plug-in",
                    m.license
                ),
            );
            if m.license_files.is_empty() {
                if l.terms == Terms::Proprietary {
                    report.reject(
                        "licence.text",
                        at,
                        "the creator's own licence needs its text in the pack (`license_files`)",
                    );
                } else {
                    report.review(
                        "licence.text",
                        at,
                        "no licence text in the pack (`license_files`)",
                    );
                }
            }
        }
        Err(e) => report.reject("licence.expression", at, e),
    }
    for f in &m.license_files {
        text_files.push(f);
        if paths::kind(f) != Some(FileKind::Text) || !files.contains(f) {
            report.reject(
                "licence.text",
                Some(f),
                "a licence text named in the manifest is not a text file in the pack",
            );
        }
    }

    // The credits.
    if m.creators.is_empty() {
        report.review(
            "credits.missing",
            at,
            "no creators credited (`[[creators]]`)",
        );
    }
    for c in &m.creators {
        if !plain(&c.name, 120) {
            report.reject(
                "credits.name",
                at,
                "a creator's name of 1 to 120 characters",
            );
        }
    }

    // The notices.
    for n in &m.notices {
        text_files.push(&n.file);
        if paths::kind(&n.file) != Some(FileKind::Text) || !files.contains(&n.file) {
            report.reject(
                "notice.file",
                Some(&n.file),
                format!("{}'s notice is not a text file in the pack", n.name),
            );
        }
        if let Err(e) = licence::parse(&n.license) {
            report.reject("notice.licence", Some(&n.file), e);
        }
    }

    // Each asset's provenance.
    let mut seen: Vec<&str> = Vec::new();
    for a in &m.assets {
        let p = Some(a.path.as_str());
        if seen.contains(&a.path.as_str()) {
            report.reject("asset.duplicate", p, "its provenance is given twice");
            continue;
        }
        seen.push(&a.path);
        let Some(bytes) = files.get(&a.path) else {
            report.reject("asset.missing", p, "named in the manifest, not in the pack");
            continue;
        };
        if let Some(h) = &a.sha256
            && !h.eq_ignore_ascii_case(&crate::sha256(bytes))
        {
            report.reject(
                "asset.hash",
                p,
                "its SHA-256 is not the manifest's: the file was changed after it was declared",
            );
        }
        if let Some(l) = &a.license
            && let Err(e) = licence::parse(l)
        {
            report.reject("asset.licence", p, e);
        }
        match a.origin {
            Origin::Original => {}
            Origin::Generated => {
                if a.tool.as_deref().is_none_or(|t| t.trim().is_empty()) {
                    report.review(
                        "provenance.tool",
                        p,
                        "generated, its tool not named: whether it can be owned and licensed \
                         depends on how it was made",
                    );
                } else {
                    report.review(
                        "provenance.generated",
                        p,
                        "generated with a tool: whether it is protected, and so licensable, \
                         differs between countries",
                    );
                }
            }
            Origin::Derived | Origin::ThirdParty => {
                if a.source.is_none() || a.license.is_none() {
                    report.review(
                        "provenance.source",
                        p,
                        "another's work, its source or its licence not named",
                    );
                }
            }
        }
    }
    for path in files.paths() {
        let untracked = path != crate::MANIFEST
            && path != crate::signature::SIGNATURE
            && !text_files.contains(&path)
            && !path.ends_with(".md")
            && m.asset(path).is_none();
        if untracked {
            report.review(
                "provenance.missing",
                Some(path),
                "no provenance in the manifest (`[[assets]]`)",
            );
        }
    }

    // The seller's declarations.
    if context.built_in {
        return;
    }
    match &m.declarations {
        None => report.review(
            "declaration.missing",
            at,
            "no seller's declarations: needed before a marketplace lists it, not to use it",
        ),
        Some(d) => {
            if !d.rights_to_distribute || !d.authorizes_customer_licensing {
                report.review(
                    "declaration.incomplete",
                    at,
                    "the seller has not declared both the right to distribute every asset under \
                     the pack's licence and the marketplace's authority to license it to customers",
                );
            }
            if !plain(&d.declared_by, 120) || !valid_date(&d.declared_on) {
                report.reject(
                    "declaration.signed",
                    at,
                    "declarations need `declared_by` and `declared_on` (YYYY-MM-DD)",
                );
            }
            report.note(
                "declaration.declared",
                at,
                format!(
                    "declared by {} on {}{}: declared, not cleared",
                    d.declared_by,
                    d.declared_on,
                    d.agreement
                        .as_deref()
                        .map(|a| format!(" under agreement {a}"))
                        .unwrap_or_default()
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_versions_and_dates() {
        assert!(valid_id("org.example.night-panel"));
        assert!(!valid_id("night"));
        assert!(!valid_id("Org.Example"));
        assert!(!valid_id("org..x"));
        assert!(!valid_id("org.-x"));
        assert!(valid_version("1.0.12"));
        assert!(!valid_version("1.0"));
        assert!(!valid_version("1.0.0-beta"));
        assert!(valid_date("2026-10-16"));
        assert!(!valid_date("16/10/2026"));
    }

    #[test]
    fn a_price_is_never_a_packs() {
        let mut r = Report::default();
        assert!(read(b"format = 1\nprice = 9.99\n", &mut r).is_none());
        assert!(r.has("manifest.store"));
    }

    #[test]
    fn a_newer_format_asks_for_a_newer_plugin() {
        let mut r = Report::default();
        assert!(read(b"format = 2\n", &mut r).is_none());
        assert!(r.findings[0].message.contains("newer plug-in"));
    }

    #[test]
    fn unknown_fields_are_refused() {
        let mut r = Report::default();
        let text = b"format = 1\nid = \"a.b\"\nname = \"N\"\nversion = \"1.0.0\"\nplugin = \"ca-72\"\nlicense = \"MIT\"\nscript = \"x.lua\"\n";
        assert!(read(text, &mut r).is_none());
        assert!(r.has("manifest.fields"));
    }
}
