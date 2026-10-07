# Vendored: CLAP

The CLAP headers the Audio Unit's wrapper (`../clap-wrapper`) is built against
(decisions.md R30). Not modified.

[free-audio/clap](https://github.com/free-audio/clap), tag `1.2.6`, commit
`69a69252fdd6ac1d06e246d9a04c0a89d9607a17` (MIT, `LICENSE`): `CMakeLists.txt`, `LICENSE`,
`README.md`, `clap.pc.in` and `include/`; the examples, artwork and CI are left out (the
manifest's targets that use them are built only with `CLAP_BUILD_TESTS`). The version
clap-wrapper 0.16.0 fetches for itself.

SHA-256 of the tree (every file but this one, sorted by path, as
`find . -type f ! -name VENDORED.md -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256`
from this directory): `67259d07eca07bb095486e09c7d177a3d4f6217ffc48c95bcb409b1bfadbbb57`.

To update: with clap-wrapper (`../clap-wrapper/PATCHES.md`).
