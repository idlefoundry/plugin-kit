//! A pack's signature (decisions.md R-SKIN-KEYS): `signature.toml` in the pack, made by Idle
//! Foundry or a future marketplace, and checked offline against the public keys built into the
//! plug-in. No login and no network: a signature proves who published these files. It is not
//! copy protection (a pack works however it was obtained) and not a licence.
//!
//! What is signed: the signature's own fields (its key's id, the pack's id and version, the
//! publisher, the date) and a digest of every file of the pack but the signature, each by its
//! path and its SHA-256. A pack changed in any way after it was signed fails its check.
//!
//! Unsigned packs load as before (anyone's packs, made and shared outside the marketplace). A
//! signature by a key the plug-in does not know (a newer key than an older plug-in has) counts
//! as none. A pack signed by Idle Foundry's key as "first-party" may use the makers' id prefix.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Deserialize;

use crate::{Files, Report};

/// The signature's name in a pack.
pub const SIGNATURE: &str = "signature.toml";

/// A public key the plug-in trusts: its id (as signatures name it) and its 32 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustedKey {
    pub id: &'static str,
    pub public: [u8; 32],
    /// May attest to Idle Foundry's own packs; a marketplace key cannot claim this.
    pub first_party: bool,
}

/// Who a signature says published the pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publisher {
    /// A seller's pack, reviewed and published by the marketplace.
    Marketplace,
    /// Idle Foundry's own pack, signed by its first-party key.
    FirstParty,
}

impl Publisher {
    fn name(self) -> &'static str {
        match self {
            Publisher::Marketplace => "marketplace",
            Publisher::FirstParty => "first-party",
        }
    }
}

/// A pack's signature, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub key: String,
    pub publisher: Publisher,
    pub signed_on: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignatureFile {
    format: u32,
    key: String,
    pack: String,
    version: String,
    publisher: String,
    signed_on: String,
    digest: String,
    signature: String,
}

/// The digest of a pack's files, the signature aside: SHA-256 over each path and its SHA-256,
/// in order.
pub fn digest(files: &Files) -> String {
    let mut listing = String::new();
    for (path, bytes) in files.iter() {
        if path == SIGNATURE {
            continue;
        }
        listing.push_str(path);
        listing.push('\0');
        listing.push_str(&crate::sha256(bytes));
        listing.push('\n');
    }
    crate::sha256(listing.as_bytes())
}

/// What is signed: every field, one a line.
fn message(
    key: &str,
    pack: &str,
    version: &str,
    publisher: &str,
    signed_on: &str,
    digest: &str,
) -> Vec<u8> {
    format!(
        "idlefoundry skin pack signature 1\nkey={key}\npack={pack}\nversion={version}\npublisher={publisher}\nsigned_on={signed_on}\ndigest={digest}\n"
    )
    .into_bytes()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// A signing key from its 32 secret bytes in hexadecimal (the owner's key file).
pub fn signing_key(secret_hex: &str) -> Option<SigningKey> {
    unhex::<32>(secret_hex).map(|b| SigningKey::from_bytes(&b))
}

/// The public key of a signing key, in hexadecimal (as a plug-in's [`TrustedKey`] holds it).
pub fn public_hex(key: &SigningKey) -> String {
    hex(key.verifying_key().as_bytes())
}

/// A pack's public key from hexadecimal.
pub fn public_key(public_hex: &str) -> Option<[u8; 32]> {
    unhex::<32>(public_hex)
}

/// The signature of the pack in `files` (its id and version its manifest's), as the text of its
/// `signature.toml`: what the publisher's tool writes into the pack.
pub fn sign(
    files: &Files,
    (pack, version): (&str, &str),
    (key_id, key): (&str, &SigningKey),
    publisher: Publisher,
    signed_on: &str,
) -> String {
    let d = digest(files);
    let m = message(key_id, pack, version, publisher.name(), signed_on, &d);
    let s = key.sign(&m);
    format!(
        "# This pack's signature proves who published these files.\n\
         # Any change to the pack breaks it.\n\
         format = 1\nkey = \"{key_id}\"\npack = \"{pack}\"\nversion = \"{version}\"\n\
         publisher = \"{}\"\nsigned_on = \"{signed_on}\"\ndigest = \"{d}\"\nsignature = \"{}\"\n",
        publisher.name(),
        hex(&s.to_bytes())
    )
}

/// Checks the pack's signature, if it has one, against `keys`: who published it, or `None`
/// (unsigned, or signed by a key not known here). A signature that does not match the pack is
/// rejected (reported).
pub fn verify(
    files: &Files,
    (pack, version): (&str, &str),
    keys: &[TrustedKey],
    report: &mut Report,
) -> Option<Signed> {
    let at = Some(SIGNATURE);
    let Some(bytes) = files.get(SIGNATURE) else {
        report.note(
            "signature.none",
            None,
            "unsigned: no trusted publisher signature (it loads all the same)",
        );
        return None;
    };
    let Some(text) = std::str::from_utf8(bytes).ok() else {
        report.reject("signature.format", at, "not UTF-8 text");
        return None;
    };
    let file: SignatureFile = match toml::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            report.reject("signature.format", at, e.to_string());
            return None;
        }
    };
    if file.format != 1 {
        report.reject(
            "signature.format",
            at,
            format!(
                "signature format {}: a newer plug-in is needed",
                file.format
            ),
        );
        return None;
    }
    let publisher = match file.publisher.as_str() {
        "marketplace" => Publisher::Marketplace,
        "first-party" => Publisher::FirstParty,
        p => {
            report.reject("signature.format", at, format!("publisher `{p}`"));
            return None;
        }
    };
    let Some(key) = keys.iter().find(|k| k.id == file.key) else {
        report.note(
            "signature.unknown-key",
            at,
            format!(
                "signed with key `{}`, which this plug-in does not know (perhaps a newer one): \
                 treated as unsigned",
                file.key
            ),
        );
        return None;
    };
    let invalid = |report: &mut Report, why: &str| {
        report.reject(
            "signature.invalid",
            at,
            format!("{why}: the pack cannot be used; get an unchanged copy from its publisher"),
        );
    };
    if publisher == Publisher::FirstParty && !key.first_party {
        invalid(report, "this key cannot sign Idle Foundry packs");
        return None;
    }
    if file.pack != pack || file.version != version {
        invalid(report, "signed for another pack or version");
        return None;
    }
    let d = digest(files);
    if !file.digest.eq_ignore_ascii_case(&d) {
        invalid(report, "its files have changed since it was signed");
        return None;
    }
    let (Some(sig), Ok(public)) = (
        unhex::<64>(&file.signature),
        VerifyingKey::from_bytes(&key.public),
    ) else {
        invalid(report, "its signature cannot be read");
        return None;
    };
    let m = message(
        &file.key,
        pack,
        version,
        &file.publisher,
        &file.signed_on,
        &d,
    );
    if public.verify(&m, &Signature::from_bytes(&sig)).is_err() {
        invalid(report, "its signature does not match");
        return None;
    }
    report.note(
        "signature.valid",
        at,
        format!(
            "signed by {} with key `{}` on {} ({})",
            if publisher == Publisher::FirstParty {
                "Idle Foundry"
            } else {
                "the marketplace"
            },
            file.key,
            file.signed_on,
            publisher.name()
        ),
    );
    Some(Signed {
        key: file.key,
        publisher,
        signed_on: file.signed_on,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Limits;

    fn files(extra: &[(&str, &[u8])]) -> Files {
        let mut base: Vec<(&str, &[u8])> = vec![
            ("skin.toml", b"format = 1\n".as_slice()),
            ("art/panel.svg", b"<svg/>".as_slice()),
        ];
        base.extend_from_slice(extra);
        let mut r = Report::default();
        Files::from_memory(base, &Limits::default(), &mut r)
    }

    /// A key for the tests (from fixed bytes: never one the plug-in trusts).
    fn test_key() -> (SigningKey, TrustedKey) {
        let k = SigningKey::from_bytes(&[7u8; 32]);
        let public = k.verifying_key().to_bytes();
        (
            k,
            TrustedKey {
                id: "test-1",
                public,
                first_party: true,
            },
        )
    }

    fn signed(publisher: Publisher) -> (Files, TrustedKey) {
        let (k, trusted) = test_key();
        let unsigned = files(&[]);
        let sig = sign(
            &unsigned,
            ("org.example.x", "1.0.0"),
            ("test-1", &k),
            publisher,
            "2026-10-16",
        );
        (files(&[(SIGNATURE, sig.as_bytes())]), trusted)
    }

    #[test]
    fn a_signed_pack_is_known_by_its_publisher() {
        let (f, key) = signed(Publisher::FirstParty);
        let mut r = Report::default();
        let s = verify(&f, ("org.example.x", "1.0.0"), &[key], &mut r).expect("signed");
        assert_eq!(s.publisher, Publisher::FirstParty);
        assert!(
            r.findings
                .iter()
                .any(|f| f.message.contains("signed by Idle Foundry"))
        );
        assert!(!r.rejected(), "{r}");
    }

    #[test]
    fn unsigned_packs_need_no_key() {
        let mut r = Report::default();
        assert!(verify(&files(&[]), ("org.example.x", "1.0.0"), &[], &mut r).is_none());
        assert!(!r.rejected() && r.has("signature.none"), "{r}");
    }

    #[test]
    fn a_key_id_is_signed_too_and_marketplace_keys_cannot_claim_idle_foundry() {
        let (f, key) = signed(Publisher::FirstParty);
        let text = std::str::from_utf8(f.get(SIGNATURE).expect("signed")).expect("text");
        let changed = files(&[(SIGNATURE, text.replace("test-1", "test-2").as_bytes())]);
        let other_id = TrustedKey {
            id: "test-2",
            ..key
        };
        let mut r = Report::default();
        assert!(verify(&changed, ("org.example.x", "1.0.0"), &[other_id], &mut r).is_none());
        assert!(r.has("signature.invalid"), "{r}");
        // An unknown id still loads as unsigned, as an older plug-in treats a newer key.
        let mut r = Report::default();
        assert!(verify(&changed, ("org.example.x", "1.0.0"), &[key], &mut r).is_none());
        assert!(!r.rejected() && r.has("signature.unknown-key"), "{r}");
        let mut r = Report::default();
        let marketplace = TrustedKey {
            first_party: false,
            ..key
        };
        assert!(verify(&f, ("org.example.x", "1.0.0"), &[marketplace], &mut r).is_none());
        assert!(r.has("signature.invalid"), "{r}");
    }

    #[test]
    fn a_changed_pack_fails_its_signature() {
        let (f, key) = signed(Publisher::Marketplace);
        let sig = f.get(SIGNATURE).expect("signed").to_vec();
        // A file changed.
        let changed = files(&[(SIGNATURE, &sig), ("extra.txt", b"x")]);
        let mut r = Report::default();
        assert!(verify(&changed, ("org.example.x", "1.0.0"), &[key], &mut r).is_none());
        assert!(r.has("signature.invalid"), "{r}");
        // Its version, or its publisher claimed otherwise.
        let mut r = Report::default();
        assert!(verify(&f, ("org.example.x", "1.0.1"), &[key], &mut r).is_none());
        assert!(r.has("signature.invalid"));
        let claimed = String::from_utf8(sig)
            .expect("text")
            .replace("\"marketplace\"", "\"first-party\"");
        let f = files(&[(SIGNATURE, claimed.as_bytes())]);
        let mut r = Report::default();
        assert!(verify(&f, ("org.example.x", "1.0.0"), &[key], &mut r).is_none());
        assert!(r.has("signature.invalid"), "{r}");
    }

    #[test]
    fn a_signature_by_another_key_is_not_trusted() {
        let (f, _) = signed(Publisher::FirstParty);
        let other = TrustedKey {
            id: "test-1",
            public: SigningKey::from_bytes(&[9u8; 32])
                .verifying_key()
                .to_bytes(),
            first_party: true,
        };
        let mut r = Report::default();
        assert!(verify(&f, ("org.example.x", "1.0.0"), &[other], &mut r).is_none());
        assert!(r.has("signature.invalid"));
        // A key not known at all: as if unsigned (an older plug-in, a newer key).
        let mut r = Report::default();
        assert!(verify(&f, ("org.example.x", "1.0.0"), &[], &mut r).is_none());
        assert!(!r.rejected() && r.has("signature.unknown-key"), "{r}");
    }
}
