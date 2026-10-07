# softbuffer, patched

The `softbuffer` crate's release 0.4.8 from crates.io, the version the plug-ins' `Cargo.lock`
took until this copy and the latest, from
[rust-windowing/softbuffer](https://github.com/rust-windowing/softbuffer) at commit
`d871852faa1137d4615b99b9ad31c3ba80f345b5` (MIT OR Apache-2.0, `LICENSE-MIT`,
`LICENSE-APACHE`): the package's `src`, `README.md`, licences and manifest (its
`Cargo.toml.orig`, as `Cargo.toml`). The benchmarks, the CI and the Cargo configuration are left
out, and so are the manifest's `[[bench]]`, `[[example]]`, `[workspace]` and dev-dependency
tables.

As copied, before those removals and the change below, the tree's SHA-256 (every file but this
one, sorted by path, as
`find . -type f ! -name PATCHES.md -print0 | sort -z | xargs -0 sha256sum | sha256sum` from
this directory) was `7dd456dac30f609950356c97f8307ee55052eaf9e1b815c96d09309db603d92b`.

A plug-in takes this copy in place of crates.io's through its workspace's
`[patch.crates-io]`, by git at the kit's commit (`docs/decisions.md` K3), with the features it
took before.

One change, for macOS only, in `src/backends/cg.rs` (`docs/decisions.md` K3; found and made
for the CA-72):

1. **Each copy of softbuffer registers an observer class of its own.** The CoreGraphics
   backend keeps its layer in step with the view's through an Objective-C object observing the
   view's layer (key-value observing). Upstream defines that object's class with objc2's
   `define_class!` under a fixed name, `SoftbufferObserver`, registered as the first surface is
   made. The Objective-C runtime has one class of a name in a process, and `define_class!`
   panics when its name is taken: "could not create new class "SoftbufferObserver", perhaps a
   class with that name already exists?". A host that loads several plug-ins into one process
   (Bitwig Studio does, for every plug-in) holds a copy of softbuffer in each plug-in's library:
   two builds of one plug-in; its CLAP and its VST3, one binary at two paths; two Idle Foundry
   plug-ins; any other plug-in with softbuffer 0.4 (0.4.6's `declare_class!` used the same
   name). The second copy to make a surface panicked; the plug-ins' editors catch softbuffer's
   panics, so the window stayed blank. Now:
   - the class is registered with objc2's `ClassBuilder`, once per copy (`observer_class`),
     under the first of `SoftbufferObserver1`, `SoftbufferObserver2`, ... that no class has.
     Never under upstream's name, which an unpatched copy loaded later still needs. Each copy
     runs only its own code, whatever the others are and whether they stay loaded;
   - the class has no instance variables: the layer an observer updates comes as the
     observation's context (the `context` of `addObserver:forKeyPath:options:context:`, null
     upstream), which `CGImpl` keeps until its `Drop` has removed the observations, as
     upstream's observer held it. The observer, `Send` and `Sync` upstream through its
     instance variables (`SendCALayer`), is so by a wrapper of its own.

   Not objc2's own way out, a name of `define_class!`'s making (leaving out `#[name]`): a copy
   that finds such a name taken uses the class it names, and so runs another copy's code, which
   objc2 itself calls unsound across libraries. The name (module path, class and the crate's
   version, `softbuffer::backends::cg::Observer0.4.8`) does not tell that code apart: upstream's
   master, changed since 0.4.8, still calls itself 0.4.8.

   The plug-ins' test `tests/softbuffer_copies.rs` (the CA-74's in `crates/ca74-plugin`, from
   the CA-72's) registers the names an unpatched copy and a patched one take, then makes a
   surface; without this change it panics as above.

To update: if a newer release no longer registers a class under a fixed name, take it from
crates.io, remove this copy and the plug-ins' `[patch.crates-io]` entries. Otherwise copy the
same files from it, remove the same tables, apply the change again, and run a plug-in's test
above and its editor on macOS.
