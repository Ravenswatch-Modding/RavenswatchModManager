# List content indicators

Before: `main` at `8135a0494997b4c7121b487ef7e65e44e884ab4a`.
After: this PR's shared list indicators and private-games badge icon changes.

Each before/after pair uses Chromium at 1280 × 800, English, the same bundled
fonts, and identical fictional fixture data. Screenshots were captured before
implementation and after implementation in the same temporary Vite harness.
The harness rendered the real components from the base/changed source; it did
not recreate their markup or CSS. It is not part of the shipped application.

- `library-*`: `RegistryListingCard` in list mode with the desktop's translated
  labels, enable switches, load-order numbers, and version metadata.
- `registry-*`: the same shared list component with native `href` links,
  category, rating, download statistics, and install action slots.
- `badges-*`: `PrivateGamesOnlyBadge` (desktop details), `ModPrivateOnlyTag`
  (website details), and a shared grid card's full private-games label.
- `private-tooltip-after` / `nsfw-tooltip-after`: the newly available list
  tooltip states. Their titles are “Private games only” and “NSFW”.

The fixtures cover private-only, NSFW-only, both restrictions, and unrestricted
mods. Midnight Wardrobe and After Dark Encounters are fictional adult-content
examples; no existing community mod was reclassified. The web detail screenshot
omits authenticated admin controls using a harness stub; those controls and
their behavior are unchanged by this PR. No live API writes were performed.

Additional browser verification covered the real desktop Library browser
preview at 1280 × 900 and 800 × 700, the shared web list at 390 × 844, translated
Chinese labels/descriptions, hover/focus/click tooltips, Escape and blur,
keyboard activation without opening the row, and native web row navigation.
The native web link's hit area covers each row while icon/action buttons remain
outside the anchor. Both content indicators remain visible at narrow widths.

Validation:

- Desktop: 501 tests passed, including content-indicator order, accessible names,
  native-link separation, grid labels, and locale coverage.
- Website: 195 tests passed.
- Shared UI, desktop, and website TypeScript checks passed.
- Desktop Vite and website Next.js production builds passed.

The rose is the official Lucide Rose artwork, backported with its ISC notice
because the currently pinned Lucide package predates it. No dependency update
is needed.
