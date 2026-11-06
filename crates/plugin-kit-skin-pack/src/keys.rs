//! The public keys shipped in the plug-in (decisions.md R-SKIN-KEYS). Never put a secret here.

use crate::signature::TrustedKey;

/// The owner's production public key, supplied on 2026-10-28 using `pack_keygen`.
/// Only the public half belongs here; no development or test key belongs in this slot.
pub const IDLE_FOUNDRY_KEY: Option<TrustedKey> = Some(TrustedKey {
    id: "idlefoundry-1",
    public: [
        222, 120, 217, 252, 170, 64, 64, 227, 24, 61, 46, 186, 215, 248, 146, 145, 157, 221, 64,
        50, 197, 56, 49, 222, 111, 24, 34, 75, 71, 65, 166, 134,
    ],
    first_party: true,
});

/// Idle Foundry's first key, then any later keys added before they are used. For a rotation,
/// keep this key and add `idlefoundry-2`; a future marketplace key has `first_party: false`.
/// Retirement or revocation reaches an installed plug-in only with an update.
pub const TRUSTED_KEYS: &[TrustedKey] = match IDLE_FOUNDRY_KEY {
    Some(key) => &[key],
    None => &[],
};

// A public release's builds (`<PLUGIN>_PUBLIC_RELEASE=1`, or the Mac's `<PLUGIN>_REQUIRE_NOTARIZATION=1`: build.rs).
#[cfg(skin_pack_release)]
const _: () = assert!(
    IDLE_FOUNDRY_KEY.is_some(),
    "Release blocked: fill plugin-kit-skin-pack::keys::IDLE_FOUNDRY_KEY with the owner's public key (the CA-72's docs/skin-packs.md, Signing first-party packs)"
);

/// The release's first-party key must be present in the shared trust list too.
pub fn check_release_key(
    first: Option<TrustedKey>,
    keys: &[TrustedKey],
) -> Result<(), &'static str> {
    let key = first.ok_or("Idle Foundry's first-party public key is missing")?;
    if key.id != "idlefoundry-1" || !key.first_party || !keys.contains(&key) {
        return Err("Idle Foundry's first-party key must be idlefoundry-1 in TRUSTED_KEYS");
    }
    let public = ed25519_dalek::VerifyingKey::from_bytes(&key.public)
        .map_err(|_| "Idle Foundry's public key is not an Ed25519 key")?;
    if public.is_weak() {
        return Err("Idle Foundry's public key is weak");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_first_party_key_passes_the_release_gate() {
        assert_eq!(check_release_key(IDLE_FOUNDRY_KEY, TRUSTED_KEYS), Ok(()));
    }

    #[test]
    fn release_requires_a_first_party_key_in_the_trust_list() {
        let key = TrustedKey {
            id: "idlefoundry-1",
            public: ed25519_dalek::SigningKey::from_bytes(&[7; 32])
                .verifying_key()
                .to_bytes(),
            first_party: true,
        };
        assert!(check_release_key(None, &[]).is_err());
        assert!(check_release_key(Some(key), &[]).is_err());
        assert!(check_release_key(Some(key), &[key]).is_ok());
        for bad in [
            TrustedKey {
                id: "wrong-id",
                ..key
            },
            TrustedKey {
                first_party: false,
                ..key
            },
            TrustedKey {
                public: [0; 32],
                ..key
            },
        ] {
            assert!(check_release_key(Some(bad), &[bad]).is_err());
        }
    }
}
