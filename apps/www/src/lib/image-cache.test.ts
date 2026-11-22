import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { registerImageCache } from './image-cache';

const source = readFileSync(new URL('../../public/image-cache-sw.js', import.meta.url), 'utf8');
const url = 'https://cdn.rsmm.me/cover.png';
const image = (body = 'image') => new Response(body, { headers: { 'Content-Type': 'image/png' } });

afterEach(() => vi.unstubAllGlobals());

it('warms images that load before and after worker control and cleans up listeners', () => {
  class ImageNode {
    complete = true;
    naturalWidth = 100;
    currentSrc = url;
    src = url;
  }
  const postMessage = vi.fn();
  const worker = Object.assign(new EventTarget(), {
    register: vi.fn(async () => undefined),
    controller: { postMessage },
  });
  const document = Object.assign(new EventTarget(), { images: [new ImageNode()] });
  vi.stubGlobal('HTMLImageElement', ImageNode);
  vi.stubGlobal('navigator', { serviceWorker: worker });
  vi.stubGlobal('document', document);
  const cleanup = registerImageCache();
  worker.dispatchEvent(new Event('controllerchange'));
  expect(postMessage).toHaveBeenLastCalledWith({ type: 'rsmm-image-warm', urls: [url] });
  const loaded = new Event('load');
  Object.defineProperty(loaded, 'target', { value: new ImageNode() });
  document.dispatchEvent(loaded);
  expect(postMessage).toHaveBeenCalledTimes(2);
  cleanup();
  document.dispatchEvent(loaded);
  expect(postMessage).toHaveBeenCalledTimes(2);
});

function harness() {
  const stores = new Map<string, Map<string, Response>>();
  const key = (request: Request | string) => (typeof request === 'string' ? request : request.url);
  const caches = {
    open: vi.fn(async (name: string) => {
      const data = stores.get(name) ?? new Map<string, Response>();
      stores.set(name, data);
      return {
        match: async (request: Request | string) => data.get(key(request))?.clone(),
        put: async (request: Request | string, response: Response) => {
          data.set(key(request), response.clone());
        },
        delete: async (request: Request | string) => data.delete(key(request)),
        keys: async () => [...data.keys()].map((url) => new Request(url)),
      };
    }),
  };
  const fetch = vi.fn(async () => image());
  const listeners = new Map<string, (event: unknown) => void>();
  runInNewContext(source, {
    self: {
      addEventListener: (type: string, listener: (event: unknown) => void) =>
        listeners.set(type, listener),
    },
    caches,
    fetch,
    Response,
    Request,
    Date,
    Map,
    Promise,
  });
  function dispatch(target = url, destination = 'image', method = 'GET') {
    const work: Promise<unknown>[] = [];
    let response: Promise<Response> | undefined;
    listeners.get('fetch')?.({
      request: { url: target, destination, method },
      waitUntil: (promise: Promise<unknown>) => work.push(promise),
      respondWith: (promise: Promise<Response>) => {
        response = promise;
      },
    });
    return {
      response,
      async done() {
        await response;
        await Promise.all(work);
      },
    };
  }
  return { stores, caches, fetch, dispatch, listeners };
}

describe('registry image worker', () => {
  it('serves a repeat visit without a network request', async () => {
    const { dispatch, fetch } = harness();
    await dispatch().done();
    const repeat = dispatch();
    expect(await (await repeat.response)?.text()).toBe('image');
    await repeat.done();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('serves stale bytes immediately while updating in the background', async () => {
    const { dispatch, fetch, stores } = harness();
    await dispatch().done();
    stores.get('rsmm-image-times-v1')?.set(url, new Response('0'));
    let finish: ((response: Response) => void) | undefined;
    fetch.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const stale = dispatch();
    expect(await (await stale.response)?.text()).toBe('image');
    finish?.(image('replacement'));
    await stale.done();
    expect(await (await dispatch().response)?.text()).toBe('replacement');
  });

  it('retains cached images offline and on server errors', async () => {
    const { dispatch, fetch, stores } = harness();
    await dispatch().done();
    stores.get('rsmm-image-times-v1')?.set(url, new Response('0'));
    fetch.mockRejectedValue(new Error('offline'));
    await dispatch().done();
    fetch.mockResolvedValue(new Response('not found', { status: 404 }));
    await dispatch().done();
    expect(await stores.get('rsmm-images-v1')?.get(url)?.text()).toBe('image');
  });

  it('handles changed URLs independently and bounds disk entries', async () => {
    const { dispatch, stores, fetch } = harness();
    for (let i = 0; i < 66; i++) await dispatch(`${url}?v=${i}`).done();
    expect(fetch).toHaveBeenCalledTimes(66);
    expect(stores.get('rsmm-images-v1')?.size).toBe(64);
    expect(stores.get('rsmm-image-times-v1')?.size).toBe(64);
  });

  it('falls back to the network if cache storage is denied', async () => {
    const { dispatch, caches } = harness();
    caches.open.mockRejectedValue(new Error('blocked'));
    expect(await (await dispatch().response)?.text()).toBe('image');
  });

  it('warms images loaded before worker control and evicts failed images', async () => {
    const { listeners, fetch, stores } = harness();
    const work: Promise<unknown>[] = [];
    const waitUntil = (promise: Promise<unknown>) => work.push(promise);
    listeners.get('message')?.({
      data: { type: 'rsmm-image-warm', urls: [url, 'data:image/png;base64,eA=='] },
      waitUntil,
    });
    await Promise.all(work);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(stores.get('rsmm-images-v1')?.has(url)).toBe(true);
    listeners.get('message')?.({ data: { type: 'rsmm-image-error', url }, waitUntil });
    await Promise.all(work);
    expect(stores.get('rsmm-images-v1')?.has(url)).toBe(false);
    expect(stores.get('rsmm-image-times-v1')?.has(url)).toBe(false);
  });

  it.each(['document', '', 'script', 'font', 'video'])(
    'does not intercept %s requests',
    (destination) => {
      const { dispatch, fetch } = harness();
      expect(dispatch(url, destination).response).toBeUndefined();
      expect(fetch).not.toHaveBeenCalled();
    },
  );

  it('does not intercept writes or cache private/error responses', async () => {
    const { dispatch, fetch, stores } = harness();
    expect(dispatch(url, 'image', 'POST').response).toBeUndefined();
    fetch.mockResolvedValue(
      new Response('private', {
        headers: { 'Content-Type': 'image/png', 'Cache-Control': 'private' },
      }),
    );
    await dispatch().done();
    expect(stores.get('rsmm-images-v1')?.size).toBe(0);
  });
});
