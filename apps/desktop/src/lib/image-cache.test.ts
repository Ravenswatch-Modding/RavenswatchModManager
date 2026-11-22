import {
  IMAGE_MAX_AGE,
  type ImageEntry,
  cacheableImageUrl,
  createImageCache,
} from '@rsmm/ui/image-cache';
import { describe, expect, it, vi } from 'vitest';

const url = 'https://cdn.rsmm.me/mod-images/test/cover.png';
const old = new Blob(['old'], { type: 'image/png' });
const fresh = new Blob(['new'], { type: 'image/png' });

function setup() {
  const entries = new Map<string, ImageEntry>();
  const store = {
    read: vi.fn(async (key: string) => entries.get(key)),
    write: vi.fn(async (entry: ImageEntry) => {
      entries.set(entry.url, entry);
    }),
  };
  const download = vi.fn(async () => fresh as Blob | undefined);
  const now = () => IMAGE_MAX_AGE * 2;
  return { entries, store, download, cache: createImageCache(store, download, now), now };
}

describe('persistent WebView images', () => {
  it('downloads at most three images at once so a full page cannot flood the network', async () => {
    const { download, cache } = setup();
    let active = 0;
    let peak = 0;
    const gates: (() => void)[] = [];
    download.mockImplementation(async () => {
      active++;
      peak = Math.max(peak, active);
      await new Promise<void>((go) => gates.push(go));
      active--;
      return fresh;
    });
    let settled = 0;
    const loads = Array.from({ length: 8 }, (_, i) =>
      cache.load(`${url}?n=${i}`, vi.fn()).finally(() => settled++),
    );
    while (settled < 8) {
      await new Promise((r) => setTimeout(r, 0));
      gates.shift()?.();
    }
    await expect(Promise.all(loads)).resolves.toEqual(Array(8).fill(fresh));
    expect(download).toHaveBeenCalledTimes(8);
    expect(peak).toBe(3);
  });

  it('reuses bytes after a new cache instance (app restart) without downloading', async () => {
    const { store, download, cache, now } = setup();
    await expect(cache.load(url, vi.fn())).resolves.toBe(fresh);
    const restarted = createImageCache(store, download, now);
    await expect(restarted.load(url, vi.fn())).resolves.toBe(fresh);
    expect(download).toHaveBeenCalledTimes(1);
  });

  it('returns stale bytes while an unresolved refresh runs, then publishes the replacement', async () => {
    const { entries, download, cache } = setup();
    entries.set(url, { url, blob: old, updatedAt: 0 });
    let finish: ((blob: Blob) => void) | undefined;
    download.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const updated = vi.fn();
    await expect(cache.load(url, updated)).resolves.toBe(old);
    expect(updated).not.toHaveBeenCalled();
    finish?.(fresh);
    await vi.waitFor(() => expect(updated).toHaveBeenCalledWith(fresh));
    expect(entries.get(url)?.blob).toBe(fresh);
  });

  it('retains stale images offline and after a corrupt/failed replacement', async () => {
    const { entries, download, cache } = setup();
    entries.set(url, { url, blob: old, updatedAt: 0 });
    download.mockRejectedValue(new Error('offline'));
    const updated = vi.fn();
    await expect(cache.load(url, updated)).resolves.toBe(old);
    download.mockResolvedValue(undefined);
    await expect(cache.load(url, updated)).resolves.toBe(old);
    expect(updated).not.toHaveBeenCalled();
    expect(entries.get(url)?.blob).toBe(old);
  });

  it('deduplicates simultaneous thumbnails and full-size views', async () => {
    const { download, cache } = setup();
    await Promise.all([cache.load(url, vi.fn()), cache.load(url, vi.fn())]);
    expect(download).toHaveBeenCalledTimes(1);
  });

  it('fetches a changed URL immediately even while the old URL is fresh', async () => {
    const { cache, download } = setup();
    await cache.load(url, vi.fn());
    await cache.load(`${url}?v=2`, vi.fn());
    expect(download).toHaveBeenCalledTimes(2);
  });

  it('continues displaying images when storage is unavailable or full', async () => {
    const { store, cache } = setup();
    store.read.mockRejectedValue(new Error('blocked'));
    store.write.mockRejectedValue(new Error('quota'));
    await expect(cache.load(url, vi.fn())).resolves.toBe(fresh);
  });

  it.each([
    'http://cdn.rsmm.me/a.png',
    'https://cdn.rsmm.me.evil.test/a.png',
    'https://evil.test/a.png',
    'https://cdn.rsmm.me:444/a.png',
    'https://user:password@cdn.rsmm.me/a.png',
    'data:image/png;base64,eA==',
    'blob:local',
    '/assets/bundled.png',
  ])('does not fetch %s through the blob cache', async (src) => {
    const { cache, download } = setup();
    expect(cacheableImageUrl(src)).toBe(false);
    await expect(cache.load(src, vi.fn())).resolves.toBeUndefined();
    expect(download).not.toHaveBeenCalled();
  });
});
