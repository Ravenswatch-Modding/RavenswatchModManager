# Launch Vanilla popover

Before: `main` at `2937515f`, captured before editing the implementation.
After: `codex/launch-vanilla-popover`.

Both pairs show the real desktop Library browser preview with identical sample
mods, expanded sidebar, signed-out state, English, and the pointer over Launch
Vanilla. No launch or game installation is simulated in these screenshots.

- `before.png` / `after.png`: 1440 × 1000, Grimoire, 100% scale, cozy density.
- `before-scaled.png` / `after-scaled.png`: 960 × 600 (the minimum native window
  dimensions), Monospace, 150% scale, compact density.

The after menu item matches the full split launch control: 247 × 40 at the
default scale and 391 × 40 at 150%. Its popover adds 4px padding and a 1px border
on each side, for a 50px total height at both scales. It sits 0.5rem below the
launch control and fits above the account strip in both captures.

Verified ArrowDown opens and focuses Launch Vanilla, Escape closes and returns
focus to Launch options, Tab closes and moves to Settings, an outside click
closes, and collapsing the sidebar closes and hides the disclosure. Native game
launch behavior is unchanged and was not exercised in the browser preview.

Capture setup: `pnpm --filter desktop dev:vite --host 127.0.0.1 --port 1422`, then
open `http://127.0.0.1:1422/?preview=library` in an isolated browser session.
Appearance values were set through the preview's `useApp.updateSettings` action;
the preview uses in-memory storage and does not change a real profile.
