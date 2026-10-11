# softbuffer, patched

The `softbuffer` crate's release 0.4.8 from crates.io, the version the plug-ins' `Cargo.lock`
took until this copy and the latest, from
[rust-windowing/softbuffer](https://github.com/rust-windowing/softbuffer) at commit
`d871852faa1137d4615b99b9ad31c3ba80f345b5` (MIT OR Apache-2.0, `LICENSE-MIT`,
`LICENSE-APACHE`): the package's `src`, `README.md`, licences and manifest (its
`Cargo.toml.orig`, as `Cargo.toml`). The benchmarks, the CI and the Cargo configuration are left
out, and so are the manifest's `[[bench]]`, `[[example]]`, `[workspace]` and dev-dependency
tables.

As copied, before those removals and the changes below, the tree's SHA-256 (every file but this
one, sorted by path, as
`find . -type f ! -name PATCHES.md -print0 | sort -z | xargs -0 sha256sum | sha256sum` from
this directory) was `7dd456dac30f609950356c97f8307ee55052eaf9e1b815c96d09309db603d92b`.

A plug-in takes this copy in place of crates.io's through its workspace's
`[patch.crates-io]`, by git at the kit's commit (`docs/decisions.md` K3), with the features it
took before.

Two changes, for macOS only, in `src/backends/cg.rs` (the first in `docs/decisions.md` K3;
found and made for the CA-72, its `docs/decisions.md` R35, R59 and R60):

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

2. **The buffer is kept from frame to frame, and shown in tiles.** Upstream made a new buffer,
   zeroed, at every `buffer_mut` (`age` always 0), and at every present gave the layer one
   image of all of it, ignoring `present_with_damage`'s damage: each frame a plug-in's editor
   turned every pixel of its window into the buffer, and Core Animation took the whole image
   again, though at most frames only a small part of the window had changed. Now:
   - the buffer (`pixels`) is `CGImpl`'s, kept until the size changes; `age` is 1 once it has
     been presented and the size has not changed since, as on Windows and X11;
   - the layer shows the buffer through sublayers, tiles of 256 by 256 pixels (`TILE`; whole
     points at a scale factor of 1 or 2, so tiles meet without a seam), made again when the
     buffer's size or the layer's scale factor changes (`tile`), the layer masking to its
     bounds as an image larger than it was not drawn past them before;
   - the tiles are laid out from the layer's top down whatever the layers over it
     (`top_down`): Core Animation flips a layer's sublayers' coordinates once for each flipped
     layer from the window's root down to it, and upstream flipped its layer always, right in a
     view of AppKit's own but undone in a flipped view's (baseview's, every plug-in editor's),
     where the tiles came out in rows bottom to top (found in GarageBand, the CA-72's release
     checks). So the layer is flipped only where the layers over it leave the coordinates
     running up, looked at again at each present;
   - `present_with_damage` gives each tile the damage touches a new image, of a copy of its
     pixels (Core Animation may read an image after the buffer has changed again); every tile
     when the tiles have just been made. `present` is `present_with_damage` with all of it.

   The CA-72's test `crates/ca72-plugin/tests/softbuffer_copies.rs` also shows a frame in
   tiles, in a view of AppKit's own and in a flipped one, and finds it drawn by AppKit
   (`cacheDisplayInRect:`) where it is, every pixel (in the flipped view it fails without
   `top_down`); presents a change of one pixel and finds a new image in that pixel's tile alone,
   the buffer kept (`age` 1, the frame in it still); and makes the tiles again on a resize.

To update: if a newer release no longer registers a class under a fixed name, take it from
crates.io, remove this copy and the plug-ins' `[patch.crates-io]` entries. Otherwise copy the
same files from it, remove the same tables, apply the changes again, and run the CA-72's
test above and a plug-in's editor on macOS. If a newer release keeps its buffer and honours
damage itself, the second change can go.
