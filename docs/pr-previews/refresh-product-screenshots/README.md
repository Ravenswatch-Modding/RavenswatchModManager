# Product screenshot refresh

Base: `main` at `2937515f`. Captured on November 21, 2026.

The website before/after pair shows the same **Inside the app** section at a
1440 × 1700 viewport. The before image was captured from the untouched base
branch (temporarily restored byte-for-byte for capture) before replacing its published images. The app image pairs compare the
old published assets with fresh captures of the current implementation; these
are a documentation refresh, not a before/after change to desktop code.

## Capture state

The new assets render the real desktop React application at 1440 × 900 CSS pixels
and device scale 1.5, producing 2160 × 1350 JPEGs at quality 88. Appearance is
English, Grimoire, 100% scale, cozy density, with the sidebar expanded. Animations
are disabled during capture. Fonts and cover artwork are fully loaded.

The development-only Library preview supplies fictional example mods and four
profiles. No game installation, native shell, account, or live install is used.
The preview's explanatory banner is hidden for the captures; app controls and
content are otherwise rendered unchanged. The images demonstrate the interface,
not the availability of these example mods in the registry.

Before capture, remove `crimson-scarlet` and `boss-rush` using the preview's
in-memory `uninstall-mod` command, and run `install-mod` for `expanded-talents`
and `balanced-dreams`, then `list`. This clears intentional conflict,
missing-dependency and update fixtures without altering production or tracked
data.

| Asset | State | Published copies |
| --- | --- | --- |
| `library.jpg` | Library, Category sort | `.github/readme/`, `apps/www/public/screens/`, `apps/docs/src/assets/app-library.jpg` |
| `list.jpg` | Library, Load order sort | `.github/readme/`, `apps/www/public/screens/` |
| `config.jpg` | Expanded Talents configuration dialog, opened from its Library row | `.github/readme/`, `apps/www/public/screens/` |
| `profiles.jpg` | Library profile picker with Co-op nights actions open | `.github/readme/`, `apps/www/public/screens/` |

All published copies of each capture are byte-identical. The website's rendered
images, Open Graph card, Twitter card and application JSON-LD use the same `v=3`
cache token. Image dimensions and aspect ratios remain unchanged.

## Reproduce

1. Run `pnpm --filter desktop dev:vite --port 1423` and open
   `http://localhost:1423/?preview=library` in a clean browser session.
2. Set the viewport and appearance above. In this development-only page,
   import `previewCommand` from `/src/dev/library-preview.ts` to apply the
   in-memory cleanup commands above. Import `useApp` from `/src/store/index.ts`
   and call `useApp.getState().updateSettings({ animations: false })`.
3. Hide only the parent of `a[href="/?preview=off"]` with `display: none`.
   Wait for `document.fonts.ready` and for images to decode.
4. Capture the four states listed above. Use the same full viewport for every
   image, with no browser chrome or developer overlays.
5. Copy each JPEG to the README and website directories, then copy `library.jpg`
   to the docs hero. Update the shared cache token on website image references
   and social/structured metadata whenever the images change again.
6. Render the website's **Inside the app** section and docs homepage, and inspect
   the actual image files at full size. Confirm all nine copies have the intended
   dimensions and matching checksums.

The README banner and docs `og.png` are brand-only graphics with no desktop UI.
Historical `docs/pr-previews/` screenshots remain historical evidence and were
not regenerated.
