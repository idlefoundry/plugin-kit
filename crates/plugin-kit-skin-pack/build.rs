// A public release's key gate: on when a release sets `<PLUGIN>_PUBLIC_RELEASE=1` or the Mac's
// `<PLUGIN>_REQUIRE_NOTARIZATION=1` (or the kit-wide `IDLEFOUNDRY_*` of either), not by Cargo's
// profile, so that CI's release and bundle builds of every change keep building while the slot
// is empty (the CA-72's decisions.md R-SKIN-KEYS; plugin-kit's K-SKIN-PACK).
const PLUGINS: &[&str] = &["IDLEFOUNDRY", "CA72", "CA74", "MC79"];

fn main() {
    println!("cargo:rustc-check-cfg=cfg(skin_pack_release)");
    let on = |name: &str| std::env::var(name).as_deref() == Ok("1");
    let mut release = false;
    for p in PLUGINS {
        for v in ["PUBLIC_RELEASE", "REQUIRE_NOTARIZATION"] {
            let name = format!("{p}_{v}");
            println!("cargo:rerun-if-env-changed={name}");
            release |= on(&name);
        }
    }
    if release {
        println!("cargo:rustc-cfg=skin_pack_release");
    }
}
