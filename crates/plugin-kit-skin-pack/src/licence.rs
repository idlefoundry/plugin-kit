//! A pack's licence, as an SPDX expression (`CC-BY-4.0`, `OFL-1.1 OR MIT`,
//! `GPL-3.0-or-later WITH Font-exception-2.0`), or its creator's own terms by reference
//! (`LicenseRef-Night-Panel-EULA`, its text in the pack). The licence is the creator's choice
//! and independent of price: an open pack may be sold, a proprietary one given away.

/// What a licence lets others do with a pack, broadly: shown to its users and to a review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terms {
    /// Open: anyone may share and change it (CC0, CC BY, CC BY-SA, the GPL, MIT, ...).
    Open,
    /// Public, with restrictions on use or change (CC's NonCommercial or NoDerivatives).
    Restricted,
    /// The creator's own terms (a `LicenseRef-`), in its licence file.
    Proprietary,
}

/// The SPDX licences a pack may name, and their terms. Others are rejected rather than
/// guessed at: a review can add one.
const KNOWN: &[(&str, Terms)] = &[
    ("CC0-1.0", Terms::Open),
    ("CC-BY-3.0", Terms::Open),
    ("CC-BY-4.0", Terms::Open),
    ("CC-BY-SA-3.0", Terms::Open),
    ("CC-BY-SA-4.0", Terms::Open),
    ("CC-BY-NC-4.0", Terms::Restricted),
    ("CC-BY-NC-SA-4.0", Terms::Restricted),
    ("CC-BY-ND-4.0", Terms::Restricted),
    ("CC-BY-NC-ND-4.0", Terms::Restricted),
    ("GPL-2.0-only", Terms::Open),
    ("GPL-2.0-or-later", Terms::Open),
    ("GPL-3.0-only", Terms::Open),
    ("GPL-3.0-or-later", Terms::Open),
    ("AGPL-3.0-only", Terms::Open),
    ("AGPL-3.0-or-later", Terms::Open),
    ("LGPL-3.0-or-later", Terms::Open),
    ("MPL-2.0", Terms::Open),
    ("MIT", Terms::Open),
    ("Apache-2.0", Terms::Open),
    ("BSD-2-Clause", Terms::Open),
    ("BSD-3-Clause", Terms::Open),
    ("ISC", Terms::Open),
    ("Unlicense", Terms::Open),
    ("OFL-1.1", Terms::Open),
    ("OFL-1.1-RFN", Terms::Open),
    ("OFL-1.1-no-RFN", Terms::Open),
    ("Bitstream-Vera", Terms::Open),
];

/// The exceptions a `WITH` may name.
const EXCEPTIONS: &[&str] = &["Font-exception-2.0"];

/// The licences under which a font may be in a pack without a review: each lets a font be
/// redistributed with other work, sold with it included, its notice kept.
const FONT_LICENCES: &[&str] = &[
    "OFL-1.1",
    "OFL-1.1-RFN",
    "OFL-1.1-no-RFN",
    "Apache-2.0",
    "CC0-1.0",
    "MIT",
    "Unlicense",
    "Bitstream-Vera",
];

/// A licence expression read: its licences' ids and terms (the least open of them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Licence {
    pub ids: Vec<String>,
    pub terms: Terms,
}

/// Whether `id` is a creator's own licence's reference.
fn is_ref(id: &str) -> bool {
    id.strip_prefix("LicenseRef-").is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 64
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    })
}

/// Reads an SPDX licence expression: ids, `AND`, `OR`, `WITH` an exception, and parentheses.
pub fn parse(expr: &str) -> Result<Licence, String> {
    if expr.trim().is_empty() || expr.len() > 256 {
        return Err("no licence, or a licence expression too long".into());
    }
    let spaced = expr.replace('(', " ( ").replace(')', " ) ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    let mut ids = Vec::new();
    let mut terms = Terms::Open;
    let mut depth = 0i32;
    // What may come next: a licence (or an opening parenthesis), or an operator (or a closing
    // one).
    let mut want_licence = true;
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        match (want_licence, t) {
            (true, "(") => depth += 1,
            (false, ")") => {
                depth -= 1;
                if depth < 0 {
                    return Err("unbalanced parentheses".into());
                }
            }
            (false, "AND" | "OR") => want_licence = true,
            (false, "WITH") => {
                let Some(exc) = tokens.get(i + 1) else {
                    return Err("`WITH` and no exception".into());
                };
                if !EXCEPTIONS.contains(exc) {
                    return Err(format!("an exception not known: `{exc}`"));
                }
                i += 1;
            }
            (true, id) => {
                let t = if is_ref(id) {
                    Terms::Proprietary
                } else if let Some((_, t)) = KNOWN.iter().find(|(k, _)| *k == id) {
                    *t
                } else {
                    return Err(format!(
                        "`{id}` is not a licence known here (an SPDX id from the list in the \
                         format's documentation, or `LicenseRef-` and a name for the creator's own)"
                    ));
                };
                terms = least_open(terms, t);
                ids.push(id.to_owned());
                want_licence = false;
            }
            (_, t) => return Err(format!("`{t}` unexpected in the licence expression")),
        }
        i += 1;
    }
    if want_licence || depth != 0 {
        return Err("the licence expression is incomplete".into());
    }
    Ok(Licence { ids, terms })
}

fn least_open(a: Terms, b: Terms) -> Terms {
    let rank = |t| match t {
        Terms::Open => 0,
        Terms::Restricted => 1,
        Terms::Proprietary => 2,
    };
    if rank(b) > rank(a) { b } else { a }
}

/// Whether a work under `a` is under `b`'s terms (the GPL's version 3 either way: a work
/// under version 3 "or later" may be passed on under version 3 alone).
pub fn same_terms(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.trim();
        match s {
            "GPL-3.0-only" | "GPL-3.0-or-later" | "GPL-3.0" | "GPL-3.0+" => "GPL-3.0".to_owned(),
            _ => s.to_owned(),
        }
    };
    norm(a) == norm(b)
}

/// Whether a font under `expr` may be in a pack without a review.
pub fn font_licence_ok(expr: &str) -> bool {
    match parse(expr) {
        // Every id in it one that permits it (an `OR` would need only one, but a review can
        // say so).
        Ok(l) => l.ids.iter().all(|id| FONT_LICENCES.contains(&id.as_str())),
        Err(_) => false,
    }
}

/// The SPDX ids known here (for the format's documentation and its tests).
pub fn known() -> impl Iterator<Item = &'static str> {
    KNOWN.iter().map(|(k, _)| *k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_are_read_with_their_terms() {
        assert_eq!(parse("CC-BY-4.0").unwrap().terms, Terms::Open);
        assert_eq!(
            parse("CC-BY-NC-4.0 OR LicenseRef-Night-EULA")
                .unwrap()
                .terms,
            Terms::Proprietary
        );
        assert_eq!(
            parse("(OFL-1.1 OR MIT) AND CC0-1.0").unwrap().ids,
            ["OFL-1.1", "MIT", "CC0-1.0"]
        );
        assert!(parse("GPL-3.0-or-later WITH Font-exception-2.0").is_ok());
    }

    #[test]
    fn malformed_or_unknown_licences_are_refused() {
        for e in [
            "",
            "Proprietary",
            "CC-BY-4.0 AND",
            "(MIT",
            "MIT)",
            "MIT OR OR CC0-1.0",
            "LicenseRef-",
            "LicenseRef-a b",
            "MIT WITH Classpath-exception-2.0",
            "GPL-3.0+ ",
        ] {
            assert!(parse(e).is_err(), "{e:?}");
        }
    }

    #[test]
    fn fonts_need_a_licence_that_permits_bundling() {
        assert!(font_licence_ok("OFL-1.1"));
        assert!(font_licence_ok("Apache-2.0"));
        assert!(!font_licence_ok("LicenseRef-Foundry-Desktop"));
        assert!(!font_licence_ok("CC-BY-NC-4.0"));
        assert!(!font_licence_ok("GPL-3.0-or-later"));
    }

    #[test]
    fn the_gpls_version_3_is_one_set_of_terms() {
        assert!(same_terms("GPL-3.0-or-later", "GPL-3.0-only"));
        assert!(!same_terms("CC-BY-4.0", "GPL-3.0-or-later"));
        assert!(!same_terms("LicenseRef-X", "GPL-3.0-or-later"));
    }
}
