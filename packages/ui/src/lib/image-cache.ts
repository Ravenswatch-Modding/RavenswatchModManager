/** Persistent image bytes for WebViews, which cannot rely on service workers. */
export const IMAGE_MAX_AGE = 24 * 60 * 60 * 1000;
const MAX_IMAGE_BYTES = 8 * 1024 * 1024;
const MAX_CACHE_BYTES = 128 * 1024 * 1024;
const MAX_ENTRIES = 256;
// Opening the app mounts every card at once; unbounded parallel downloads of
// multi-MB screenshots saturated home wifi (router RTT 5 ms -> 3 s).
const MAX_PARALLEL_DOWNLOADS = 3;

export interface ImageEntry {
  url: string;
  blob: Blob;
  updatedAt: number;
}

export interface ImageStore {
  read(url: string): Promise<ImageEntry | undefined>;
  write(entry: ImageEntry): Promise<void>;
}

// Keep this aligned with desktop img-src. A blob URL must never bypass the
// image host policy just because connect-src happens to permit a host.
export function cacheableImageUrl(src: string): boolean {
  try {
    const url = new URL(src);
    return (
      url.protocol === 'https:' &&
      !url.username &&
      !url.password &&
      !url.port &&
      ([
        'api.rsmm.me',
        's3-rsmm.me',
        'cdn.rsmm.me',
        's3-ravenswatch.ovilli.de',
        'cdn.discordapp.com',
        'avatars.githubusercontent.com',
      ].includes(url.hostname) ||
        url.hostname.endsWith('.googleusercontent.com'))
    );
  } catch {
    return false;
  }
}

let database: Promise<IDBDatabase> | undefined;
function openDatabase(): Promise<IDBDatabase> {
  if (!database) {
    database = new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open('rsmm-images', 1);
      request.onupgradeneeded = () => {
        const store = request.result.createObjectStore('images', { keyPath: 'url' });
        store.createIndex('updatedAt', 'updatedAt');
      };
      request.onsuccess = () => {
        const db = request.result;
        db.onversionchange = () => {
          db.close();
          database = undefined;
        };
        resolve(db);
      };
      request.onerror = () => reject(request.error);
      request.onblocked = () => reject(new Error('Image cache is blocked'));
    }).catch((error) => {
      database = undefined;
      throw error;
    });
  }
  return database;
}

export const imageStore: ImageStore = {
  async read(url) {
    const db = await openDatabase();
    return new Promise((resolve, reject) => {
      const tx = db.transaction('images', 'readonly');
      const request = tx.objectStore('images').get(url);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
      tx.onabort = () => reject(tx.error);
    });
  },
  async write(entry) {
    const db = await openDatabase();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('images', 'readwrite');
      const store = tx.objectStore('images');
      store.put(entry);
      // Newest first; evict old entries within the same serialized transaction.
      let bytes = 0;
      let count = 0;
      const cursor = store.index('updatedAt').openCursor(null, 'prev');
      cursor.onsuccess = () => {
        const row = cursor.result;
        if (!row) return;
        bytes += (row.value as ImageEntry).blob.size;
        count++;
        if (bytes > MAX_CACHE_BYTES || count > MAX_ENTRIES) row.delete();
        row.continue();
      };
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  },
};

async function downloadImage(url: string): Promise<Blob | undefined> {
  const response = await fetch(url, {
    credentials: 'omit',
    // Default HTTP caching: an expired entry revalidates with the stored ETag
    // (304) instead of downloading every image again.
    signal: AbortSignal.timeout(15_000),
  });
  if (
    !response.ok ||
    !response.headers.get('content-type')?.startsWith('image/') ||
    Number(response.headers.get('content-length')) > MAX_IMAGE_BYTES
  ) {
    await response.body?.cancel();
    return;
  }
  // Bound streamed bodies too: Content-Length can be absent or inaccurate.
  const reader = response.body?.getReader();
  if (!reader) return;
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    bytes += value.byteLength;
    if (bytes > MAX_IMAGE_BYTES) {
      await reader.cancel();
      return;
    }
    chunks.push(value);
  }
  const blob = new Blob(chunks, { type: response.headers.get('content-type') ?? '' });
  if (!blob.size) return;
  // A successful HTTP response can still contain corrupt image data. Never
  // replace a working offline copy until the replacement actually decodes.
  const objectUrl = URL.createObjectURL(blob);
  try {
    const image = new Image();
    image.src = objectUrl;
    await image.decode();
    return blob;
  } finally {
    URL.revokeObjectURL(objectUrl);
  }
}

export function createImageCache(store: ImageStore, download = downloadImage, now = Date.now) {
  const pending = new Map<string, Promise<Blob | undefined>>();
  let active = 0;
  const queue: (() => void)[] = [];
  async function limited(url: string): Promise<Blob | undefined> {
    if (active >= MAX_PARALLEL_DOWNLOADS) await new Promise<void>((go) => queue.push(go));
    active++;
    try {
      return await download(url);
    } finally {
      active--;
      queue.shift()?.();
    }
  }
  function refresh(url: string): Promise<Blob | undefined> {
    const existing = pending.get(url);
    if (existing) return existing;
    const request = (async () => {
      try {
        const blob = await limited(url);
        if (!blob) return;
        // Storage denial or quota exhaustion must not break a usable image.
        await store.write({ url, blob, updatedAt: now() }).catch(() => {});
        return blob;
      } catch {
        return undefined;
      } finally {
        pending.delete(url);
      }
    })();
    pending.set(url, request);
    return request;
  }
  return {
    async load(url: string, onRefresh: (blob: Blob) => void): Promise<Blob | undefined> {
      if (!cacheableImageUrl(url)) return;
      const cached = await store.read(url).catch(() => undefined);
      if (cached?.blob?.size) {
        if (now() - cached.updatedAt >= IMAGE_MAX_AGE) {
          void refresh(url).then((blob) => {
            if (blob) onRefresh(blob);
          });
        }
        return cached.blob;
      }
      return refresh(url);
    },
  };
}

export const imageCache = createImageCache(imageStore);
