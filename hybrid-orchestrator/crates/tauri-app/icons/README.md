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

Regeneration is **deferred** — it was **not** run in the offline sandbox because
no SVG rasterizer and no Tauri CLI are available there, and the network is
blocked (npm and crates.io return 403), so none can be installed. Probed and
confirmed absent: `cargo tauri` (no such command), `rsvg-convert`, `inkscape`,
ImageMagick `convert`/`magick`, Python `cairosvg` and `PIL`/Pillow, and Node
`sharp` / `@resvg/resvg-js`. A hand-rolled/approximate rasterizer that cannot
faithfully reproduce the SVG's gradients, stroked anchor/network marks, and arc
paths was intentionally **not** used, so no degraded icon was committed.

The current PNG/ICO/ICNS files are therefore still the earlier Phase 0
**placeholder** rasters (solid-color squares, not final branding). They are left
**untouched** and remain referenced by `../tauri.conf.json`, so `cargo tauri
build` has valid, non-empty image files to bundle and CI stays green.

**Action required:** the anchor artwork in `agent-harbor.svg` must be rasterized
into the bundled icon set by the gated `tauri-bundle` CI job (which has the Tauri
CLI) or by a local `cargo tauri icon <png-export-of-agent-harbor.svg>` run, using
the regeneration steps above. Until then the desktop window/taskbar icon shows
the placeholder rather than the anchor logo.
