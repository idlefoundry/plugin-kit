# The CA-72 skin pack starter

A plain pack to start your own from: every control of the CA-72's panel in three rows on a slate
face, the lamps and POWER in the wooden strip. Its layout is its own, not the official panel's.

It is dedicated to the public domain under CC0 1.0 (`LICENSE.txt`). Copy it, change it, give it
away or sell what you make from it, under any licence you choose.

1. Copy this folder and rename it.
2. In `skin.toml`, give it your own `id` (lower case, dotted: `org.yourname.your-pack`), `name`,
   `license` and `[[creators]]`, and replace `LICENSE.txt` with your licence's text.
3. Move controls, legends and rules in `layout.toml`, draw over the panel in `art/panel.svg`,
   or set `base = "realistic"` and replace its pictures (`[look.pictures]`).
4. Check it, and look at it:
   `cargo run --release -p ca72-panel --example check_pack -- <your folder> preview.png`
5. Install it: copy the folder into Idle Foundry's `Skins` folder, then choose it with the
   editor's settings key, under LOOK.

The whole format, its limits and its licensing rules: `docs/skin-packs.md` in the CA-72's
repository. Don't start from the official panel's layout or pictures unless your pack is under
the GPL: they are.
