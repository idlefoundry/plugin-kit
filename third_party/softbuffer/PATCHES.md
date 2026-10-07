# softbuffer, patched

The `softbuffer` crate's release 0.4.8 from crates.io, the version the plug-ins' `Cargo.lock`
took until this copy and the latest, from
[rust-windowing/softbuffer](https://github.com/rust-windowing/softbuffer) at commit
`d871852faa1137d4615b99b9ad31c3ba80f345b5` (MIT OR Apache-2.0, `LICENSE-MIT`,
`LICENSE-APACHE`): the package's `src`, `README.md`, licences and manifest (its
`Cargo.toml.orig`, as `Cargo.toml`). The benchmarks, the CI and the Cargo configuration are left
out, and so are the manifest's `[[bench]]`, `[[example]]`, `[workspace]` and dev-dependency
tables.

As copied, before those removals, the tree's SHA-256 (every file but this one, sorted by path,
as `find . -type f ! -name PATCHES.md -print0 | sort -z | xargs -0 sha256sum | sha256sum` from
this directory) was `7dd456dac30f609950356c97f8307ee55052eaf9e1b815c96d09309db603d92b`.

No change yet.
