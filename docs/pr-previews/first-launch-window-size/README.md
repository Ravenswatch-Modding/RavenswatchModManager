# First-launch window size

Before: `main` at `edcf6fd7`, exported into a separate source directory.
After: this PR's default window dimensions.

Both screenshots show the real desktop Library browser preview at
`/?preview=library&view=list`, with the same fictional mods, English, default
appearance and bundled fonts, expanded sidebar, and scroll position. Both browser sessions have
no saved `rsmm.window.main` geometry.

The viewport matches the corresponding `tauri.conf.json` window dimensions:

- `before.png`: 1280 × 800 at 1× scale.
- `after.png`: 1366 × 768 at 1× scale.

The differing viewport sizes are the change being reviewed. Screenshots are
uncropped browser captures; they show the resulting layout, not a native Tauri
startup. Native Windows/Linux startup was not exercised on this macOS host.

Validation: both PNG dimensions match their respective configurations; neither
platform-specific config overrides the main window dimensions. All nine existing
window-state tests pass, and Biome passes for the two changed application files.
Saved-size and position restoration code is unchanged.
