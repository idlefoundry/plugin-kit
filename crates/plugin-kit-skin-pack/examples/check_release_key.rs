//! The release gate's diagnostic, before building the bundles (decisions.md R-SKIN-KEYS).

fn main() {
    if let Err(why) = plugin_kit_skin_pack::keys::check_release_key(
        plugin_kit_skin_pack::keys::IDLE_FOUNDRY_KEY,
        plugin_kit_skin_pack::keys::TRUSTED_KEYS,
    ) {
        eprintln!("Release blocked: {why}. See docs/skin-packs.md, Signing first-party packs.");
        std::process::exit(1);
    }
    println!("Idle Foundry's first-party public key is ready for release.");
}
