import { useCallback, useEffect, useRef, useState, memo } from 'react';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ChevronLeftIcon from 'lucide-react/dist/esm/icons/chevron-left.mjs';
import { invoke } from '@tauri-apps/api/core';
import {
  featuredWebSources,
  formatRelativePublishedAt,
  type WebSource,
} from './webSources';

/** In-memory cache so scrolling / re-renders don't re-fetch og:image. */
const previewImageCache = new Map<string, string | null>();

async function openExternalUrl(url: string) {
  try {
    const { openUrl } = await import('@tauri-apps/plugin-opener');
    await openUrl(url);
  } catch {
    window.open(url, '_blank', 'noopener,noreferrer');
  }
}

async function resolvePreviewImage(pageUrl: string): Promise<string | null> {
  if (previewImageCache.has(pageUrl)) {
    return previewImageCache.get(pageUrl) ?? null;
  }
  try {
    const image = await invoke<string | null>('resolve_page_image', { url: pageUrl });
    previewImageCache.set(pageUrl, image);
    return image;
  } catch {
    previewImageCache.set(pageUrl, null);
    return null;
  }
}

function SourceCard({ source }: { source: WebSource }) {
  const [resolvedImage, setResolvedImage] = useState<string | null>(
    source.imageUrl || previewImageCache.get(source.url) || null,
  );
  const [imageFailed, setImageFailed] = useState(false);
  const when = formatRelativePublishedAt(source.publishedAt);

  useEffect(() => {
    if (source.imageUrl) {
      setResolvedImage(source.imageUrl);
      setImageFailed(false);
      return;
    }
    const cached = previewImageCache.get(source.url);
    if (cached !== undefined) {
      setResolvedImage(cached);
      return;
    }
    let cancelled = false;
    void resolvePreviewImage(source.url).then((image) => {
      if (!cancelled) {
        setResolvedImage(image);
        setImageFailed(false);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [source.imageUrl, source.url]);

  const onClick = useCallback(() => {
    void openExternalUrl(source.url);
  }, [source.url]);

  const showCover = !!resolvedImage && !imageFailed;

  return (
    <button type="button" className="web-source-card" onClick={onClick} title={source.url}>
      <div className={`web-source-card-thumb${showCover ? ' has-cover' : ''}`}>
        {showCover ? (
          <img
            className="web-source-card-cover"
            src={resolvedImage!}
            alt=""
            loading="lazy"
            referrerPolicy="no-referrer"
            onError={() => setImageFailed(true)}
          />
        ) : source.faviconUrl ? (
          <img
            className="web-source-card-favicon-large"
            src={source.faviconUrl}
            alt=""
            loading="lazy"
            referrerPolicy="no-referrer"
          />
        ) : (
          <div className="web-source-card-thumb-fallback" />
        )}
      </div>
      <div className="web-source-card-body">
        <div className="web-source-card-site">
          {source.faviconUrl && (
            <img
              className="web-source-card-favicon"
              src={source.faviconUrl}
              alt=""
              loading="lazy"
              referrerPolicy="no-referrer"
            />
          )}
          <span>{source.siteName || 'Web'}</span>
        </div>
        <div className="web-source-card-title">{source.title}</div>
        {when && <div className="web-source-card-meta">{when}</div>}
      </div>
    </button>
  );
}

export const FeaturedWebCards = memo(function FeaturedWebCards({
  sources,
}: {
  sources: WebSource[];
}) {
  const featured = featuredWebSources(sources);
  const featuredUrls = featured.map((s) => s.url).join('|');
  const scrollerRef = useRef<HTMLDivElement>(null);

  const scrollBy = useCallback((dir: 1 | -1) => {
    const el = scrollerRef.current;
    if (!el) return;
    el.scrollBy({ left: dir * 220, behavior: 'smooth' });
  }, []);

  // Prefetch og:images for featured cards as soon as the strip mounts.
  useEffect(() => {
    for (const source of featured) {
      if (source.imageUrl || previewImageCache.has(source.url)) continue;
      void resolvePreviewImage(source.url);
    }
  }, [featured, featuredUrls]);

  if (featured.length === 0) return null;

  return (
    <div className="web-source-cards">
      <div className="web-source-cards-track" ref={scrollerRef}>
        {featured.map((source) => (
          <SourceCard key={`${source.callId}:${source.url}`} source={source} />
        ))}
      </div>
      {featured.length > 2 && (
        <>
          <button
            type="button"
            className="web-source-cards-nav web-source-cards-nav-left"
            onClick={() => scrollBy(-1)}
            aria-label="Previous sources"
          >
            <ChevronLeftIcon size={14} />
          </button>
          <button
            type="button"
            className="web-source-cards-nav web-source-cards-nav-right"
            onClick={() => scrollBy(1)}
            aria-label="More sources"
          >
            <ChevronRightIcon size={14} />
          </button>
        </>
      )}
    </div>
  );
});
