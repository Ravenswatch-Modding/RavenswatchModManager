# Persistent sign-in

Before: `main` at `2937515f`.
After: the session persistence changes in this PR.

Both pairs use a 1280 × 900 viewport, the same fictional Session Tester
account, English, and the default appearance. The desktop is the existing
Library browser preview with its sample mods; the registry is `/account`.

For each version, sign in through both real email forms, advance the local API
clock by two days, and reload each page. Before, both clients lose their
session. After, the account remains available on both clients. Explicit
sign-out followed by a reload was also checked on both clients.

The local API runs the real Better Auth configuration and Hono middleware
with Better Auth's memory adapter, a stubbed ban lookup, disabled social
providers, and stubbed mail. No live accounts or database were used. Only the
server clock is advanced for the screenshots; the integration tests also
model browser cookie expiration and discarding session-only cookies at
restart. The temporary preview server was not shipped.

These are browser captures, not a native Windows/Linux relaunch test.
The desktop OAuth handoff is covered separately by the integration suite.

- `desktop-before.png` / `desktop-after.png`: Library account strip after returning.
- `registry-before.png` / `registry-after.png`: account page after returning.

Automated coverage in `apps/api/test/session-persistence.test.ts` exercises
email sign-in, the desktop one-time-token exchange, returning after two days,
hourly renewal, survival beyond the original 30-day deadline, promotion of a
still-valid legacy 24-hour session, CDN exclusion for renewed cookies,
inactivity expiry, server expiry, and explicit sign-out/revocation.
