# Full-page Settings

Before: `main` at `4d18a606a1d5da9dc2e6b43d99ca82f089d9ca28`.
After: the implementation in this PR, applied to that base.

All screenshots use a 1440 × 1050 viewport, English, the default Grimoire
appearance at 100% UI scale and Cozy density, a signed-out session, and the same
Library preview fixtures. The base was exported with `git archive`, installed
with the frozen lockfile and scripts disabled, and served independently from
the proposed branch. No production data or user game files were used.

- `before-general.png` / `after-general.png`: the app navigation and horizontal
  tabs are replaced by a dedicated Settings sidebar; sections sit directly on
  the page. The logo stays at its original position.
- `before-appearance.png` / `after-appearance.png`: consistent active navigation,
  heading hierarchy, and unboxed appearance options.
- `after-back-hover.png`: hovering the Settings heading reveals Back. Keyboard
  focus uses the same treatment. Both this control and the logo leave Settings.
- `before-library-return.png` / `after-library-return.png`: search Library for
  Scarlet, enter Settings, then return. Before uses the Library navigation link
  and loses the search; After uses Back and retains the two matching mods.

The base shows the existing preview-only banner and version footer; Settings
intentionally hides those in the proposed full-page layout. The actual desktop
React components were captured in Chromium without a native Tauri shell. Native
window dragging, window controls, and game commands were not executed.
