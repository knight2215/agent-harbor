# App icons

## Canonical source

`agent-harbor.svg` is the **canonical logo source** for the desktop app icon.
It is a hand-authored, square, self-contained SVG (harbor-anchor fused with an
agent-node/network glyph, teal/blue accent) centered and padded so it reduces
cleanly to 32x32. It shares its motif with the frontend logo at
`../../../frontend/src/assets/logo.svg`.

## Raster set (generated)

The raster app-icon set is **generated** from a high-resolution raster export of
`agent-harbor.svg` and is referenced by `bundle.icon` in `../tauri.conf.json`:

- `32x32.png`, `128x128.png`, `128x128@2x.png` (256x256) — RGBA PNGs
- `icon.ico` — Windows icon
- `icon.icns` — macOS icon

### How to regenerate

Export `agent-harbor.svg` to a high-resolution square PNG (e.g. 1024x1024) with
any SVG rasterizer, then run the Tauri CLI from a single source image:

```sh
cargo tauri icon path/to/agent-harbor.png
```

That command produces the correctly sized/optimized PNG, ICO, and ICNS assets
and overwrites the files above.

### Status (offline sandbox)

Regeneration was **not** run in the offline sandbox because the Tauri CLI is
unavailable there. The current PNG/ICO/ICNS files are the earlier Phase 0
**placeholder** rasters (solid-color squares, not final branding); they remain
in place and are still referenced by `../tauri.conf.json` so `cargo tauri build`
has valid image files to bundle and the build stays green. Regenerating the
raster set from `agent-harbor.svg` via the command above is a documented
follow-up.
