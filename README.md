# Install → Library regression captures

- Before: main at 2937515ffda44ce90c92d1220879c4d121af38ec.
- After: 88c859d800a223e359e564a870a9fdcadfb783f6.
- Viewport: 1360 × 950.
- Environment: desktop frontend Library browser preview; fictional sample mods and in-memory downloads, no game files changed.
- Profile: Co-op nights, initially five mods, including disabled Team Health Bars.
- Installed mod: Hyper Aggro (its sample manifest defaults to disabled).

Before: visit Library to populate its query cache, open Browse, run the production installModIntoProfile helper for Hyper Aggro, return to Library, and refresh the disk listing. Returning replays the old cache, drops Hyper Aggro from the profile, and the refreshed listing shows the Add to profile banner.

After: reset the preview, select the same profile, supply the Hyper Aggro sample listing to Browse, click its actual Install button, and return to Library. Hyper Aggro remains in the profile and enabled, with no Add to profile banner. Team Health Bars remains disabled.

This branch only stores review images and is not intended for merging.
