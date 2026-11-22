/** All native image requests, including server-rendered prose, share this cache. */
export function registerImageCache(): () => void {
  if (!('serviceWorker' in navigator)) return () => {};
  void navigator.serviceWorker.register('/image-cache-sw.js', { scope: '/' }).catch(() => {});
  const warm = () => {
    navigator.serviceWorker.controller?.postMessage({
      type: 'rsmm-image-warm',
      urls: [...document.images]
        .filter((image) => image.complete && image.naturalWidth > 0)
        .map((image) => image.currentSrc || image.src),
    });
  };
  navigator.serviceWorker.addEventListener('controllerchange', warm);
  // A lazy image can start before control and finish after window.load.
  const onLoad = (event: Event) => {
    if (!(event.target instanceof HTMLImageElement)) return;
    navigator.serviceWorker.controller?.postMessage({
      type: 'rsmm-image-warm',
      urls: [event.target.currentSrc || event.target.src],
    });
  };
  const onError = (event: Event) => {
    if (!(event.target instanceof HTMLImageElement)) return;
    navigator.serviceWorker.controller?.postMessage({
      type: 'rsmm-image-error',
      url: event.target.currentSrc || event.target.src,
    });
  };
  document.addEventListener('error', onError, { capture: true });
  document.addEventListener('load', onLoad, { capture: true });
  return () => {
    navigator.serviceWorker.removeEventListener('controllerchange', warm);
    document.removeEventListener('load', onLoad, { capture: true });
    document.removeEventListener('error', onError, { capture: true });
  };
}
