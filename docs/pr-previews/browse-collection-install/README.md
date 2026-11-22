# Browse collection installation

Before: untouched `origin/main` at `2937515f`.
After: this branch, using the same 1440 × 1000 viewport, default appearance,
expanded sidebar, and signed-out library browser preview.

- `before.png`: Browse → Collections on the base commit, after all cover images loaded.
- `after.png`: the same public collection cards, with Quality of Life hovered to
  reveal the Install label. Other cards retain the shared compact plus action.
- `installing.png`: the action stays on Browse, disables repeat activation, and
  shows a spinner with a compact completed/total count on hover. The accessible
  name and tooltip describe the full installation progress.
- `installed.png`: completion shows the shared Installed indicator. The success
  toast also names the collection and destination profile.
- `error.png`: a failed install leaves a dismissible error and a usable retry action.

The before/after card metadata and art came from the public registry. For the
interaction captures, the Quality of Life detail response was replaced in the
browser with three existing library-preview sample mods (ravens-toolkit,
expanded-talents, scarlet-reimagined). They were removed from the in-memory
preview folder before installation. The detail fetch was held pending to capture
the progress state. The Test collection used an unavailable preview mod to show
failure, then an available sample mod to verify retry. This exercised the real
collection button and batch installer without downloading or writing game files.
No preview fixtures or runtime interception were added to production code.

Also verified keyboard Enter and Space activate only the install action, ordinary
card activation still opens collection details, 960 × 800 layout, and 150% UI scale
without horizontal overflow or clipped progress counts. Installation into the
captured profile, partial failure/retry, folder-id reconciliation, and scan failures
are covered by the profile-mods tests. Native CLI downloads were not exercised.
