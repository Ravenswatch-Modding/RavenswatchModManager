# Shared Grimoire tokens

`@rsmm/ui/tokens.css` is the common foundation for desktop and web. Import it
before Tailwind. It defines custom properties and `color-scheme: dark`; it does
not load fonts or style layout/components.

- Desktop receives it through `@rsmm/ui/styles.css`.
- Web imports it directly in `apps/www/src/app/globals.css`.
- `tailwind.preset.cjs` exposes the utility classes used by both hosts.

Change foundational values in `src/tokens.css`, never in a host's `:root`.
Desktop font preferences, scaling, density, white title ink and window chrome
stay in the desktop styles. Web font loading, wordmark, decorative card
treatments and page layout stay in the web styles. Shared registry components
continue to own their specific treatment. This extraction does not replace
those component contracts.

## Roles and compatibility

| Role | CSS token | Existing names / utilities |
| --- | --- | --- |
| Primary text | `--text-primary` | `--foreground`, parchment |
| Secondary text | `--text-secondary` | `--muted-foreground`, smoke |
| Tertiary text | `--text-tertiary` | ash |
| Error / destructive-action text | `--text-danger` | `text-danger`; `text-crimson` remains an alias |
| Page | `--surface-page` | `--background`, `--surface-0` |
| Panel / rail | `--surface-panel` | `--surface-1` |
| Raised surface | `--surface-raised` | `--surface-2` |
| Primary action fill | `--action-primary-bg` | `--primary`, crimson |
| Destructive action fill | `--action-danger-bg` | `--destructive`, `bg-destructive` |
| Keyboard focus | `--focus-ring`, `--focus-width`, `--focus-offset` | gilt; `--ring`; 2px outline, 2px offset |

Use `text-danger` at full opacity for errors and destructive text actions.
`text-destructive` uses a dark fill and must not be used as text. Keep
`text-destructive-foreground` on a filled destructive button; it is a different
role. Never brighten `--destructive` to fix an error label.

The existing radius and icon-control tokens also live here without value
changes. Geometry and primitive API normalization are separate work.

## Contrast contract

Ratios below use full-opacity text and the declared flat/composited surfaces.
Actual overlays, artwork and reduced opacity need their own checks.

| Surface | Primary | Secondary | Tertiary | Danger |
| --- | ---: | ---: | ---: | ---: |
| Page | 15.10 | 8.71 | 6.00 | 6.30 |
| Panel | 14.06 | 8.12 | 5.59 | 5.87 |
| Raised | 13.14 | 7.58 | 5.22 | 5.48 |
| Error wash (`destructive/10` over raised) | 12.37 | 7.14 | 4.91 | 5.16 |
| Selected row (`char/55` over raised) | 9.67 | 5.58 | — | — |

Selected rows use primary or secondary ink. Tertiary and danger ink are not
approved on that stronger selection fill. Gilt focus clears 3:1 on every
surface above. The translucent web card is also covered by the regression test.

Run `pnpm --filter desktop test src/lib/theme-tokens.test.ts` for the token
contract, generated Tailwind mappings and contrast matrix. Run both app builds
and inspect their actual rendered styles when changing imports or host rules.

## Dialog behavior

Use `Dialog` from `@rsmm/ui` for modal content in either host. Both host stylesheets
already import `@rsmm/ui/dialog.css`. Mount it while the task is open, or control
its `open` prop. Supply `aria-labelledby` pointing to the visible title (or an
`aria-label` for an image viewer), and `aria-describedby` for a short explanation.

`Dialog` uses `showModal()` for native background inertness and top-layer stacking,
wraps Tab/Shift+Tab within the active dialog, locks page scrolling until the last
shared dialog closes, and restores the opening control before unmount. Use
`initialFocusRef` for a field, a reading target with `tabIndex={-1}`, or Cancel on
a destructive confirmation. The desktop `useDialog()` adapter handles prompt and
confirmation results; Enter on Cancel must never submit the task.

`onDismiss` owns Escape. Omit it for a notice that must be answered; do not add a
window-level Escape listener. Cards and backdrop click policy stay with the host.
Use `overlay` for the existing full-window backdrop/card layout, or keep a custom
native-dialog skin such as `registry-filter-dialog`. Bound tall cards to the
viewport and give their content a scroller. Render interactive popovers inside
the dialog rather than portalling them to the inert page behind it.

Run `pnpm --filter desktop exec playwright install chromium` once, then
`pnpm --filter desktop test:dialogs`. The browser suite uses actual production
components with local fixtures, checks keyboard/stacking/scroll behavior in
React StrictMode, and intercepts API requests. CI runs this suite in Chromium
on Ubuntu 24.04, which is supported by the pinned Playwright version. Native
Tauri WebViews and assistive technology still need platform verification.
