/* Image-only cache: HTML, API responses, auth and downloads always use the network. */
const CACHE_NAME = 'rsmm-images-v1';
const META_CACHE_NAME = 'rsmm-image-times-v1';
const MAX_AGE = 24 * 60 * 60 * 1000;
// Opaque cross-origin responses can consume ~7 MB of browser quota each.
const MAX_ENTRIES = 64;
const MAX_IMAGE_BYTES = 8 * 1024 * 1024;
const pending = new Map();
let writes = Promise.resolve();

self.addEventListener('install', (event) => event.waitUntil(self.skipWaiting()));
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));

async function remember(request, response) {
  const cache = await caches.open(CACHE_NAME);
  const times = await caches.open(META_CACHE_NAME);
  // Cache.put replaces atomically: quota failure must retain the old response.
  await cache.put(request, response);
  await times.put(request, new Response(String(Date.now())));
  const keys = await cache.keys();
  for (const key of keys.slice(0, Math.max(0, keys.length - MAX_ENTRIES))) {
    await cache.delete(key);
    await times.delete(key);
  }
}

function refresh(request, cacheMode = 'no-cache') {
  const existing = pending.get(request.url);
  if (existing) return existing.then((response) => response.clone());
  const work = (async () => {
    // Prefer readable responses so a CDN 404/500 cannot replace good bytes.
    // External Markdown hosts may not support CORS; preserve native images
    // there using an opaque response, with page error eviction below.
    let response;
    try {
      response = await fetch(request, { cache: cacheMode, mode: 'cors', credentials: 'omit' });
    } catch {
      response = await fetch(request, { cache: cacheMode });
    }
    const opaque = response.type === 'opaque';
    // Image destinations include SVG, CSS backgrounds, srcset and Markdown.
    // Never cache a readable error page or a private/no-store response.
    if (
      (opaque || (response.ok && response.headers.get('content-type')?.startsWith('image/'))) &&
      !/no-store|private/i.test(response.headers.get('cache-control') ?? '') &&
      Number(response.headers.get('content-length')) <= MAX_IMAGE_BYTES
    ) {
      const copy = response.clone();
      writes = writes.catch(() => {}).then(() => remember(request, copy));
      await writes.catch(() => {});
    }
    return response;
  })().finally(() => pending.delete(request.url));
  pending.set(request.url, work);
  return work.then((response) => response.clone());
}

async function imageResponse(request, event) {
  let cached;
  let updatedAt = 0;
  try {
    const cache = await caches.open(CACHE_NAME);
    const times = await caches.open(META_CACHE_NAME);
    [cached, updatedAt] = await Promise.all([
      cache.match(request),
      times.match(request).then(async (response) => Number(await response?.text()) || 0),
    ]);
  } catch {
    // Private browsing / quota restrictions: retain normal browser behavior.
  }
  if (cached) {
    if (Date.now() - updatedAt >= MAX_AGE) {
      event.waitUntil(refresh(request).catch(() => {}));
    }
    return cached;
  }
  return refresh(request);
}

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET' || request.destination !== 'image' || !/^https?:/.test(request.url))
    return;
  // Extend lifetime synchronously, including the asynchronous cache lookup.
  const response = imageResponse(request, event);
  event.respondWith(response);
  event.waitUntil(response.then(() => writes).catch(() => {}));
});

// Opaque responses hide HTTP status. An image error in the page evicts them
// (and corrupt readable images) so a transient 404 cannot poison future visits.
self.addEventListener('message', (event) => {
  // Images parsed before the worker first took control are already in the
  // HTTP cache. Copy those too, without requiring another page visit.
  if (event.data?.type === 'rsmm-image-warm' && Array.isArray(event.data.urls)) {
    event.waitUntil(
      (async () => {
        const cache = await caches.open(CACHE_NAME);
        for (const url of event.data.urls.slice(0, MAX_ENTRIES)) {
          if (typeof url !== 'string' || !/^https?:\/\//.test(url)) continue;
          const request = new Request(url, { mode: 'no-cors', credentials: 'omit' });
          if (!(await cache.match(request))) await refresh(request, 'force-cache').catch(() => {});
        }
      })().catch(() => {}),
    );
    return;
  }
  if (event.data?.type !== 'rsmm-image-error' || typeof event.data.url !== 'string') return;
  event.waitUntil(
    Promise.all(
      [CACHE_NAME, META_CACHE_NAME].map(async (name) => {
        const cache = await caches.open(name);
        await cache.delete(event.data.url);
      }),
    ).catch(() => {}),
  );
});
