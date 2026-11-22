import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Real Better Auth handlers and Hono middleware; only persistence and mail are
// replaced. No live database, accounts, providers or email are used.
process.env.DATABASE_URL ??= 'postgres://user:pass@localhost:5432/db';
process.env.BETTER_AUTH_SECRET ??= 'session-persistence-test-secret-32-characters';

vi.mock('../src/env.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../src/env.js')>();
  return {
    ...actual,
    isProduction: false,
    smtpConfigured: () => false,
    googleConfigured: () => false,
    discordConfigured: () => false,
    githubConfigured: () => false,
    env: {
      ...actual.env,
      betterAuthUrl: 'http://localhost:3001',
      betterAuthSecret: 'session-persistence-test-secret-32-characters',
    },
  };
});
vi.mock('../src/mailer.js', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../src/mailer.js')>()),
  sendMail: vi.fn(),
}));
vi.mock('@rsmm/db', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@rsmm/db')>()),
  getDb: () => ({
    select: () => ({
      from: () => ({ where: () => ({ limit: () => Promise.resolve([]) }) }),
    }),
  }),
}));
vi.mock('better-auth/adapters/drizzle', async () => {
  const { memoryAdapter } = await import('better-auth/adapters/memory');
  return {
    drizzleAdapter: () => memoryAdapter({ user: [], session: [], account: [], verification: [] }),
  };
});

const { app } = await import('../src/app.js');
const { getAuth } = await import('../src/auth.js');
const DAY = 86_400_000;
let requestNumber = 0;

/** Browser-like jar: Max-Age advances with the clock; reloads keep cookies. */
class CookieJar {
  private cookies = new Map<string, { value: string; expires: number }>();

  receive(response: Response): void {
    for (const cookie of response.headers.getSetCookie()) {
      const [pair = ''] = cookie.split(';');
      const separator = pair.indexOf('=');
      const name = pair.slice(0, separator);
      const maxAge = cookie.match(/;\s*Max-Age=(-?\d+)/i)?.[1];
      this.cookies.set(name, {
        value: pair.slice(separator + 1),
        expires:
          maxAge === undefined ? Number.POSITIVE_INFINITY : Date.now() + Number(maxAge) * 1000,
      });
    }
  }

  header(): string {
    return [...this.cookies]
      .filter(([, cookie]) => cookie.expires > Date.now())
      .map(([name, cookie]) => `${name}=${cookie.value}`)
      .join('; ');
  }

  restart(): void {
    for (const [name, cookie] of this.cookies) {
      if (!Number.isFinite(cookie.expires)) this.cookies.delete(name);
    }
  }
}

async function request(path: string, jar: CookieJar, body?: object): Promise<Response> {
  const response = await app.request(`http://localhost:3001${path}`, {
    method: body ? 'POST' : 'GET',
    headers: {
      cookie: jar.header(),
      origin: 'http://localhost:3000',
      'content-type': 'application/json',
      // Isolate rate-limit counters across cases (including parallel CI runs).
      'x-real-ip': `session-test-${++requestNumber}`,
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  jar.receive(response);
  return response;
}

async function signIn(): Promise<CookieJar> {
  const jar = new CookieJar();
  const response = await request('/api/auth/sign-up/email', jar, {
    name: 'Session Tester',
    email: `${crypto.randomUUID()}@example.test`,
    password: 'Local-test-password-123!',
  });
  expect(response.status).toBe(200);
  expect(jar.header()).toContain('better-auth.session_token=');
  return jar;
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(new Date('2026-01-01T12:00:00Z'));
});
afterEach(() => vi.useRealTimers());

describe('persistent sign-in on desktop and registry', () => {
  it('survives a fresh client returning two days later with only persistent cookies', async () => {
    const jar = await signIn();
    jar.restart();
    vi.setSystemTime(Date.now() + 2 * DAY);
    const response = await request('/api/auth/get-session', jar);
    expect(response.status).toBe(200);
    expect(await response.json()).toMatchObject({ user: { name: 'Session Tester' } });
  });

  it('forwards the hourly renewal before the original cookie expires', async () => {
    const jar = await signIn();
    vi.setSystemTime(Date.now() + 2 * 60 * 60 * 1000);
    const refreshed = await request('/api/me', jar);
    expect(refreshed.status).toBe(200);
    expect(
      refreshed.headers
        .getSetCookie()
        .some(
          (cookie) =>
            cookie.startsWith('better-auth.session_token=') && cookie.includes('HttpOnly'),
        ),
    ).toBe(true);
  });

  it.each(['/api/auth/get-session', '/api/me', '/api/auth-config'])(
    'delivers renewal from %s so the cookie outlives its original deadline',
    async (path) => {
      const jar = await signIn();
      vi.setSystemTime(Date.now() + 29 * DAY);
      const refreshed = await request(path, jar);
      expect(refreshed.status).toBe(200);
      const cookie = refreshed.headers
        .getSetCookie()
        .find((value) => value.startsWith('better-auth.session_token='));
      expect(cookie).toContain('Max-Age=2592000');
      expect(cookie).toContain('HttpOnly');
      expect(refreshed.headers.get('cache-control')).toContain('no-store');
      expect(refreshed.headers.get('vercel-cdn-cache-control')).toBeNull();
      vi.setSystemTime(Date.now() + 2 * DAY);
      const reloaded = await request('/api/auth/get-session', jar);
      expect(await reloaded.json()).toMatchObject({ user: { name: 'Session Tester' } });
    },
  );

  it('renews an existing 24-hour session before its old deadline', async () => {
    const jar = await signIn();
    const auth = await getAuth();
    const session = await auth.api.getSession({ headers: new Headers({ cookie: jar.header() }) });
    expect(session).not.toBeNull();
    if (!session) throw new Error('Expected a signed-in session');
    await (await auth.$context).internalAdapter.updateSession(session.session.token, {
      expiresAt: new Date(Date.now() + DAY),
    });
    const refreshed = await request('/api/auth/get-session', jar);
    expect(refreshed.headers.getSetCookie().join(';')).toContain('Max-Age=2592000');
    vi.setSystemTime(Date.now() + 2 * DAY);
    expect(await (await request('/api/auth/get-session', jar)).json()).not.toBeNull();
  });

  it('keeps the desktop OAuth cookie when a fresh client returns two days later', async () => {
    const browser = await signIn();
    const { token } = await (await getAuth()).api.generateOneTimeToken({
      headers: new Headers({ cookie: browser.header() }),
    });
    const desktop = new CookieJar();
    const exchange = await request('/api/auth/one-time-token/verify', desktop, { token });
    expect(exchange.status).toBe(200);
    expect(exchange.headers.getSetCookie().join(';')).toContain('Max-Age=2592000');
    desktop.restart();
    vi.setSystemTime(Date.now() + 2 * DAY);
    expect(await (await request('/api/auth/get-session', desktop)).json()).toMatchObject({
      user: { name: 'Session Tester' },
    });
  });

  it('persists an email sign-in across restarts too', async () => {
    const jar = await signIn();
    const session = await (await getAuth()).api.getSession({
      headers: new Headers({ cookie: jar.header() }),
    });
    if (!session) throw new Error('Expected a signed-in session');
    await request('/api/auth/sign-out', jar, {});
    const response = await request('/api/auth/sign-in/email', jar, {
      email: session.user.email,
      password: 'Local-test-password-123!',
      rememberMe: true,
    });
    expect(response.status).toBe(200);
    jar.restart();
    vi.setSystemTime(Date.now() + 2 * DAY);
    expect(await (await request('/api/auth/get-session', jar)).json()).toMatchObject({
      user: { id: session.user.id },
    });
  });

  it('clears a cookie whose server session has expired instead of reviving it', async () => {
    const jar = await signIn();
    const auth = await getAuth();
    const session = await auth.api.getSession({ headers: new Headers({ cookie: jar.header() }) });
    if (!session) throw new Error('Expected a signed-in session');
    await (await auth.$context).internalAdapter.updateSession(session.session.token, {
      expiresAt: new Date(Date.now() - 1),
    });
    expect(await (await request('/api/auth/get-session', jar)).json()).toBeNull();
    expect(jar.header()).not.toContain('better-auth.session_token=');
  });

  it('still expires after 30 days of inactivity', async () => {
    const jar = await signIn();
    vi.setSystemTime(Date.now() + 31 * DAY);
    expect(await (await request('/api/auth/get-session', jar)).json()).toBeNull();
    expect((await request('/api/me', jar)).status).toBe(401);
  });

  it('sign-out clears the cookie even when renewal is due and revokes the session', async () => {
    const jar = await signIn();
    const oldCookie = jar.header();
    vi.setSystemTime(Date.now() + 2 * 60 * 60 * 1000);
    expect((await request('/api/auth/sign-out', jar, {})).status).toBe(200);
    expect(jar.header()).not.toContain('better-auth.session_token=');
    expect(await (await request('/api/auth/get-session', jar)).json()).toBeNull();
    const replay = await (await getAuth()).api.getSession({
      headers: new Headers({ cookie: oldCookie }),
    });
    expect(replay).toBeNull();
  });
});
