# Application icon sources

- `AppIcon.icon` is the macOS 26+ Icon Composer source. Its SVG artwork is
  square and unmasked so the system owns the final silhouette and material.
  It includes explicit dark and mono/tinted artwork variants so the rings and
  strands remain recognizable when macOS applies those icon styles.
- `threestrands-icon.svg` is an unmasked, flat reference composite of those
  layers.
- `threestrands-icon-legacy.svg` is the compatibility source for `.icns`,
  Windows, Linux, and older macOS releases that can't render Icon Composer
  assets. It includes the conventional optical inset and rounded silhouette.

The two postmark rings and three cancellation waves use heavier strokes than
the original artwork so that the mark remains distinct at 16 and 32 pixels.

`pnpm tauri` runs through `scripts/tauri.mjs`, which supplies a macOS-only
`actool` stdin workaround required by Tauri CLI 2.11.4. Remove the shim after
tauri-apps/tauri#15991 is released and the CLI dependency is updated.
