'use client';

import { createContext, useContext, useEffect, useRef, useState } from 'react';
import type { ComponentProps, ReactNode, SyntheticEvent } from 'react';
import {
  type ThumbnailWidth,
  cacheableImageUrl,
  imageCache,
  thumbnailUrl,
} from '../lib/image-cache';

const ImageCacheContext = createContext(false);

/** The website caches native image requests in its worker; WebViews use IDB. */
export function ImageCacheProvider({ children }: { children: ReactNode }) {
  return <ImageCacheContext.Provider value>{children}</ImageCacheContext.Provider>;
}

export function CachedImage({
  thumb,
  ...props
}: ComponentProps<'img'> & {
  /** Show a resized copy this wide; falls back to the original if it fails. */
  thumb?: ThumbnailWidth;
}) {
  const original = props.src;
  const thumbnail =
    thumb && typeof original === 'string' ? thumbnailUrl(original, thumb) : original;
  const [failedThumb, setFailedThumb] = useState<string>();
  if (thumbnail === original || failedThumb === thumbnail) return <PlainImage {...props} />;
  return (
    <PlainImage
      {...props}
      key={thumbnail}
      src={thumbnail}
      onError={() => setFailedThumb(thumbnail)}
    />
  );
}

function PlainImage({ src, alt, loading, onError, ...props }: ComponentProps<'img'>) {
  const enabled = useContext(ImageCacheContext);
  const cached = enabled && typeof src === 'string' && cacheableImageUrl(src);
  const element = useRef<HTMLImageElement>(null);
  const [resolved, setResolved] = useState<{ source: string; url: string }>();

  useEffect(() => {
    if (!cached || !src) return;
    let disposed = false;
    const urls: string[] = [];
    const show = (blob: Blob | undefined) => {
      if (disposed) return;
      const url = blob ? URL.createObjectURL(blob) : src;
      if (blob) urls.push(url);
      setResolved({ source: src, url });
    };
    const load = () => void imageCache.load(src, show).then(show);
    let observer: IntersectionObserver | undefined;
    if (loading === 'lazy' && element.current && typeof IntersectionObserver !== 'undefined') {
      observer = new IntersectionObserver(
        (entries) => {
          if (entries.some((entry) => entry.isIntersecting)) {
            observer?.disconnect();
            load();
          }
        },
        { rootMargin: '300px' },
      );
      observer.observe(element.current);
    } else load();
    return () => {
      disposed = true;
      observer?.disconnect();
      for (const url of urls) URL.revokeObjectURL(url);
    };
  }, [cached, src, loading]);

  const displaySrc = cached ? (resolved?.source === src ? resolved.url : undefined) : src;
  // A server-rendered <img> can fail before React hydrates, and React never
  // replays that error event. Without this, a failed thumbnail on the website
  // never falls back to the original and the card stays blank.
  // biome-ignore lint/correctness/useExhaustiveDependencies: run once per source; onError is a fresh closure each render
  useEffect(() => {
    const img = element.current;
    if (!cached && img?.complete && img.naturalWidth === 0 && img.getAttribute('src'))
      onError?.({ currentTarget: img, target: img } as unknown as SyntheticEvent<HTMLImageElement>);
  }, [displaySrc]);
  return (
    <img
      {...props}
      alt={alt}
      ref={element}
      loading={loading}
      src={displaySrc}
      onError={(event) => {
        // CORS, private mode and a corrupt disk entry must still allow the
        // ordinary <img> path and the caller's existing unavailable state.
        if (cached && src && displaySrc !== src) setResolved({ source: src, url: src });
        else onError?.(event);
      }}
    />
  );
}
