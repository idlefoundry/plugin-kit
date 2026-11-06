//! Signs a folder or `.ifskin` with a built-in key (decisions.md R-SKIN-KEYS):
//! `cargo run -p skin-pack --example pack_sign -- <pack> <secret-file|-> <key-id>
//! <first-party|marketplace> <YYYY-MM-DD>`. `-` reads the secret from stdin, never an argument.
//! Checks the signed result against the built-in keys before replacing the signature or archive.

use std::io::{Read, Write};
use std::path::Path;

use plugin_kit_skin_pack::signature::{self, Publisher, TrustedKey};
use plugin_kit_skin_pack::{Context, Files, Limits, Pack, Report};
use zeroize::Zeroizing;

fn main() {
    if let Err(why) = run() {
        eprintln!("pack_sign: {why}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, key_file, key_id, publisher, date] = args.as_slice() else {
        return Err("pack_sign <folder or .ifskin> <secret-key-file|-> <key-id> <first-party|marketplace> <YYYY-MM-DD>".into());
    };
    if path.starts_with('-') {
        return Err("a pack path is required".into());
    }
    let publisher = match publisher.as_str() {
        "marketplace" => Publisher::Marketplace,
        "first-party" => Publisher::FirstParty,
        _ => return Err("the publisher is `first-party` or `marketplace`".into()),
    };
    if date.len() != 10
        || !date.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return Err("the date must be YYYY-MM-DD".into());
    }
    let secret = if key_file == "-" {
        read_secret(std::io::stdin().lock())?
    } else {
        let file = std::fs::File::open(key_file)
            .map_err(|e| format!("cannot open the secret file: {e}"))?;
        read_secret(file)?
    };
    let key = signature::signing_key(&secret).ok_or("the secret must be 64 hexadecimal digits")?;
    let files = sign_pack(
        Path::new(path),
        key_id,
        &key,
        publisher,
        date,
        plugin_kit_skin_pack::keys::TRUSTED_KEYS,
    )?;
    let path = Path::new(path);
    if plugin_kit_skin_pack::archive::is_pack_file(path) {
        let bytes = plugin_kit_skin_pack::archive::write(&files)?;
        replace(path, &bytes)?;
    } else {
        replace(
            &path.join(signature::SIGNATURE),
            files.get(signature::SIGNATURE).ok_or("no signature")?,
        )?;
    }
    println!(
        "Signed with `{key_id}` and verified against the built-in keys: {}",
        path.display()
    );
    Ok(())
}

fn read_secret(input: impl Read) -> Result<Zeroizing<String>, String> {
    let mut text = Zeroizing::new(String::new());
    input
        .take(4097)
        .read_to_string(&mut text)
        .map_err(|_| "cannot read the secret as text")?;
    if text.len() > 4096 || signature::signing_key(&text).is_none() {
        return Err("the secret must be 64 hexadecimal digits (from a file or stdin)".into());
    }
    Ok(text)
}

fn sign_pack(
    path: &Path,
    key_id: &str,
    key: &ed25519_dalek::SigningKey,
    publisher: Publisher,
    date: &str,
    keys: &[TrustedKey],
) -> Result<Files, String> {
    let trusted = keys
        .iter()
        .find(|k| k.id == key_id)
        .ok_or("the key id is not built in; fill the public key slot and rebuild the tool first")?;
    if trusted.public != key.verifying_key().to_bytes() {
        return Err("the secret does not match that built-in public key".into());
    }
    if publisher == Publisher::FirstParty && !trusted.first_party {
        return Err("this key cannot sign Idle Foundry packs".into());
    }
    let limits = Limits::default();
    let mut report = Report::default();
    let files = if plugin_kit_skin_pack::archive::is_pack_file(path) {
        plugin_kit_skin_pack::archive::read_file(path, &limits, &mut report)
    } else {
        Files::read_dir(path, &limits, &mut report)
    };
    if report.rejected() {
        return Err(report.to_string());
    }
    let manifest = plugin_kit_skin_pack::manifest::read(
        files
            .get(plugin_kit_skin_pack::MANIFEST)
            .unwrap_or_default(),
        &mut report,
    )
    .ok_or_else(|| report.to_string())?;
    let text = signature::sign(
        &files,
        (&manifest.id, &manifest.version),
        (key_id, key),
        publisher,
        date,
    );
    let signed = Files::from_memory(
        files
            .iter()
            .filter(|(p, _)| *p != signature::SIGNATURE)
            .chain([(signature::SIGNATURE, text.as_bytes())]),
        &limits,
        &mut report,
    );
    let context = Context {
        plugin: &manifest.plugin,
        official: &[],
        official_licence: "",
        built_in: false,
        test_drawings: true,
        keys,
        plugin_version: None,
    };
    let pack = Pack::check(signed, report, &limits, &context).map_err(|r| r.to_string())?;
    if pack.signed.is_none() {
        return Err("the signed result was not verified by a built-in key".into());
    }
    Ok(pack.files)
}

/// Same-folder replacement: a failed write never leaves half a signature or half an archive.
fn replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(bytes)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> (ed25519_dalek::SigningKey, TrustedKey) {
        let key = signature::signing_key(&"07".repeat(32)).expect("a throwaway key");
        let trusted = TrustedKey {
            id: "test-1",
            public: key.verifying_key().to_bytes(),
            first_party: true,
        };
        (key, trusted)
    }

    #[test]
    fn secrets_are_read_from_a_stream_and_bad_or_excessive_input_is_refused() {
        assert!(read_secret(format!("{}\n", "07".repeat(32)).as_bytes()).is_ok());
        assert!(read_secret(b"not a key".as_slice()).is_err());
        assert!(read_secret(" ".repeat(4097).as_bytes()).is_err());
    }

    #[test]
    fn folders_and_archives_are_signed_and_verified_before_any_write() {
        let dir = tempfile::tempdir().expect("a folder");
        let starter = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/starter");
        let (key, trusted) = test_key();
        let mut report = Report::default();
        let files = Files::read_dir(&starter, &Limits::default(), &mut report);
        let archive = dir.path().join("starter.ifskin");
        std::fs::write(
            &archive,
            plugin_kit_skin_pack::archive::write(&files).expect("an archive"),
        )
        .expect("written");
        for path in [&starter, &archive] {
            let signed = sign_pack(
                path,
                trusted.id,
                &key,
                Publisher::FirstParty,
                "2026-10-18",
                &[trusted],
            )
            .expect("signed");
            let signed_by = signature::verify(
                &signed,
                ("org.example.ca72-starter", "1.0.0"),
                &[trusted],
                &mut report,
            )
            .expect("verified");
            assert_eq!(signed_by.publisher, Publisher::FirstParty);
            assert!(
                sign_pack(
                    path,
                    "wrong-id",
                    &key,
                    Publisher::FirstParty,
                    "2026-10-18",
                    &[trusted]
                )
                .is_err()
            );
            assert!(
                sign_pack(
                    path,
                    trusted.id,
                    &key,
                    Publisher::FirstParty,
                    "2026-10-18",
                    &[]
                )
                .is_err()
            );
            let other = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
            assert!(
                sign_pack(
                    path,
                    trusted.id,
                    &other,
                    Publisher::FirstParty,
                    "2026-10-18",
                    &[trusted]
                )
                .is_err()
            );
            let out = dir.path().join("signed.ifskin");
            replace(
                &out,
                &plugin_kit_skin_pack::archive::write(&signed).expect("an archive"),
            )
            .expect("saved");
            let read =
                plugin_kit_skin_pack::archive::read_file(&out, &Limits::default(), &mut report);
            assert_eq!(signed, read);
        }
        assert!(!starter.join(signature::SIGNATURE).exists());
    }
}
