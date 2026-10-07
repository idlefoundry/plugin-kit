# Vendored: AudioUnitSDK

Apple's base classes for Audio Units (version 2), which the Audio Unit's wrapper
(`../clap-wrapper`) builds on (decisions.md R30). Not modified.

[apple/AudioUnitSDK](https://github.com/apple/AudioUnitSDK), tag `AudioUnitSDK-1.1.0`,
commit `53a9a2008aae7fb1b0a9f093dd523b9b12f6c0d9` (Apache License 2.0, `LICENSE.txt`):
`LICENSE.txt`, `readme.md`, `include/` and `src/`; the Xcode project, demos, tests and tools
are left out. The version clap-wrapper 0.16.0 fetches for itself.

SHA-256 of the tree (every file but this one, sorted by path, as
`find . -type f ! -name VENDORED.md -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256`
from this directory): `8a49a7e53a5e0f8fe4518671e1ab35a232a0117cf820df87ab5c58a9ce7a86bd`.

To update: with clap-wrapper (`../clap-wrapper/PATCHES.md`).
