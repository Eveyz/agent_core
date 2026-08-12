import { useCallback, useRef, useState, memo } from 'react';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import ChevronLeftIcon from 'lucide-react/dist/esm/icons/chevron-left.mjs';
import MapPinIcon from 'lucide-react/dist/esm/icons/map-pin.mjs';
import RouteIcon from 'lucide-react/dist/esm/icons/route.mjs';
import { useTranslation } from 'react-i18next';
import {
  featuredMapFeatures,
  providerLabel,
  type MapFeature,
  type MapPlace,
  type MapRoute,
} from './mapSources';
import { InteractiveMapView } from './InteractiveMapView';

async function openExternalUrl(url: string) {
  try {
    const { openUrl } = await import('@tauri-apps/plugin-opener');
    await openUrl(url);
  } catch {
    window.open(url, '_blank', 'noopener,noreferrer');
  }
}

function PlaceCard({ place }: { place: MapPlace }) {
  const [imageFailed, setImageFailed] = useState(false);
  const showCover = !!place.previewUrl && !imageFailed;
  const onClick = useCallback(() => {
    void openExternalUrl(place.mapUrl);
  }, [place.mapUrl]);

  return (
    <button type="button" className="map-feature-card" onClick={onClick} title={place.mapUrl}>
      <div className={`map-feature-card-thumb${showCover ? ' has-cover' : ''}`}>
        {showCover ? (
          <img
            className="map-feature-card-cover"
            src={place.previewUrl}
            alt=""
            loading="lazy"
            referrerPolicy="no-referrer"
            onError={() => setImageFailed(true)}
          />
        ) : (
          <div className="map-feature-card-thumb-fallback">
            <MapPinIcon size={28} />
          </div>
        )}
      </div>
      <div className="map-feature-card-body">
        <div className="map-feature-card-provider">{providerLabel(place.provider)}</div>
        <div className="map-feature-card-title">{place.name}</div>
        {place.address && <div className="map-feature-card-meta">{place.address}</div>}
      </div>
    </button>
  );
}

function RouteCard({ route }: { route: MapRoute }) {
  const [imageFailed, setImageFailed] = useState(false);
  const showCover = !!route.previewUrl && !imageFailed;
  const onClick = useCallback(() => {
    void openExternalUrl(route.mapUrl);
  }, [route.mapUrl]);

  return (
    <button type="button" className="map-feature-card" onClick={onClick} title={route.mapUrl}>
      <div className={`map-feature-card-thumb${showCover ? ' has-cover' : ''}`}>
        {showCover ? (
          <img
            className="map-feature-card-cover"
            src={route.previewUrl}
            alt=""
            loading="lazy"
            referrerPolicy="no-referrer"
            onError={() => setImageFailed(true)}
          />
        ) : (
          <div className="map-feature-card-thumb-fallback">
            <RouteIcon size={28} />
          </div>
        )}
      </div>
      <div className="map-feature-card-body">
        <div className="map-feature-card-provider">{providerLabel(route.provider)}</div>
        <div className="map-feature-card-title">{route.title}</div>
        {route.summary && <div className="map-feature-card-meta">{route.summary}</div>}
      </div>
    </button>
  );
}

export const FeaturedMapCards = memo(function FeaturedMapCards({
  features,
}: {
  features: MapFeature[];
}) {
  const { t } = useTranslation();
  const featured = featuredMapFeatures(features);
  const scrollerRef = useRef<HTMLDivElement>(null);

  const scrollBy = useCallback((dir: 1 | -1) => {
    const el = scrollerRef.current;
    if (!el) return;
    el.scrollBy({ left: dir * 220, behavior: 'smooth' });
  }, []);

  if (featured.length === 0) return null;

  return (
    <div className="map-feature-block">
      <InteractiveMapView features={featured} />
      <div className="map-feature-cards">
        <div className="map-feature-cards-track" ref={scrollerRef}>
          {featured.map((feature) =>
            feature.kind === 'place' ? (
              <PlaceCard key={feature.id} place={feature} />
            ) : (
              <RouteCard key={feature.id} route={feature} />
            ),
          )}
        </div>
        {featured.length > 2 && (
          <>
            <button
              type="button"
              className="map-feature-cards-nav map-feature-cards-nav-left"
              onClick={() => scrollBy(-1)}
              aria-label={t('chat.turn.mapsPrev')}
            >
              <ChevronLeftIcon size={14} />
            </button>
            <button
              type="button"
              className="map-feature-cards-nav map-feature-cards-nav-right"
              onClick={() => scrollBy(1)}
              aria-label={t('chat.turn.mapsNext')}
            >
              <ChevronRightIcon size={14} />
            </button>
          </>
        )}
      </div>
    </div>
  );
});
