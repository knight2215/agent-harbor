# App icons

These are **placeholder** icons for the Phase 0 scaffold, referenced by
`bundle.icon` in `../tauri.conf.json` so `cargo tauri build` has real, valid
image files to bundle. They are solid-color squares, not real branding.

Files:

- `32x32.png`, `128x128.png`, `128x128@2x.png` (256x256) — RGBA PNGs
- `icon.ico` — Windows icon (32x32, 32bpp)
- `icon.icns` — macOS icon (embeds 128x128 + 256x256 PNGs)

**Replace before release.** Once real artwork exists, regenerate the full icon
set with the Tauri CLI from a single high-resolution source, e.g.:

```sh
cargo tauri icon path/to/app-icon.png
```

That command (which requires the Tauri CLI and was unavailable in the offline
Phase 0 sandbox) produces the correctly sized/optimized PNG, ICO, and ICNS
assets and overwrites these placeholders.
