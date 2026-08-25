import { useEffect, useRef, memo } from 'react';
import L from 'leaflet';
import 'leaflet/dist/leaflet.css';
import markerIconUrl from 'leaflet/dist/images/marker-icon.png';
import markerIcon2xUrl from 'leaflet/dist/images/marker-icon-2x.png';
import markerShadowUrl from 'leaflet/dist/images/marker-shadow.png';
import {
  googleMapsEmbedUrl,
  prefersGoogleEmbed,
  type MapFeature,
} from './mapSources';

const markerIcon = L.icon({
  iconUrl: markerIconUrl,
  iconRetinaUrl: markerIcon2xUrl,
  shadowUrl: markerShadowUrl,
  iconSize: [25, 41],
  iconAnchor: [12, 41],
  popupAnchor: [1, -34],
  shadowSize: [41, 41],
});

function collectPoints(features: MapFeature[]): {
  markers: Array<{ lat: number; lng: number; label: string; url: string }>;
  lines: Array<Array<[number, number]>>;
} {
  const markers: Array<{ lat: number; lng: number; label: string; url: string }> = [];
  const lines: Array<Array<[number, number]>> = [];

  for (const f of features) {
    if (f.kind === 'place') {
      if (f.lat !== undefined && f.lng !== undefined) {
        markers.push({ lat: f.lat, lng: f.lng, label: f.name, url: f.mapUrl });
      }
      continue;
    }
    if (f.polyline && f.polyline.length >= 2) {
      lines.push(f.polyline);
      const start = f.polyline[0];
      const end = f.polyline[f.polyline.length - 1];
      markers.push({ lat: start[0], lng: start[1], label: f.title.split('→')[0]?.trim() || 'Start', url: f.mapUrl });
      markers.push({ lat: end[0], lng: end[1], label: f.title.split('→')[1]?.trim() || 'End', url: f.mapUrl });
    } else if (
      f.originLat !== undefined &&
      f.originLng !== undefined &&
      f.destLat !== undefined &&
      f.destLng !== undefined
    ) {
      lines.push([
        [f.originLat, f.originLng],
        [f.destLat, f.destLng],
      ]);
      markers.push({
        lat: f.originLat,
        lng: f.originLng,
        label: f.title.split('→')[0]?.trim() || 'Start',
        url: f.mapUrl,
      });
      markers.push({
        lat: f.destLat,
        lng: f.destLng,
        label: f.title.split('→')[1]?.trim() || 'End',
        url: f.mapUrl,
      });
    }
  }

  return { markers, lines };
}

function GoogleEmbedMap({ feature }: { feature: MapFeature }) {
  const src = googleMapsEmbedUrl(feature);
  if (!src) return null;
  const title = feature.kind === 'place' ? feature.name : feature.title;
  return (
    <iframe
      className="map-interactive map-interactive-frame"
      title={title}
      src={src}
      key={src}
      loading="lazy"
      referrerPolicy="no-referrer-when-downgrade"
      allowFullScreen
    />
  );
}

function LeafletMapView({ features }: { features: MapFeature[] }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<L.Map | null>(null);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const { markers, lines } = collectPoints(features);
    if (markers.length === 0 && lines.length === 0) return;

    if (!mapRef.current) {
      mapRef.current = L.map(el, {
        zoomControl: true,
        attributionControl: true,
      });
      L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', {
        maxZoom: 19,
        attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>',
      }).addTo(mapRef.current);
    }

    const map = mapRef.current;
    const layer = L.layerGroup().addTo(map);
    const bounds = L.latLngBounds([]);

    for (const m of markers) {
      const marker = L.marker([m.lat, m.lng], { icon: markerIcon });
      marker.bindPopup(
        `<div class="map-leaflet-popup"><strong>${escapeHtml(m.label)}</strong><br/><a href="${escapeAttr(m.url)}" target="_blank" rel="noreferrer">Open map</a></div>`,
      );
      marker.addTo(layer);
      bounds.extend([m.lat, m.lng]);
    }

    for (const line of lines) {
      const poly = L.polyline(line, {
        color: '#2563eb',
        weight: 4,
        opacity: 0.85,
      });
      poly.addTo(layer);
      for (const pt of line) bounds.extend(pt);
    }

    if (bounds.isValid()) {
      map.fitBounds(bounds.pad(0.2), { maxZoom: 15, animate: false });
    } else {
      map.setView([20, 0], 2);
    }

    requestAnimationFrame(() => map.invalidateSize());

    return () => {
      layer.remove();
    };
  }, [features]);

  useEffect(() => {
    return () => {
      if (mapRef.current) {
        mapRef.current.remove();
        mapRef.current = null;
      }
    };
  }, []);

  const { markers, lines } = collectPoints(features);
  if (markers.length === 0 && lines.length === 0) return null;

  return <div className="map-interactive" ref={containerRef} role="img" aria-label="Map" />;
}

/**
 * Google Maps embed when the features came from Google MCP.
 * Leaflet/OSM is the fallback (Amap, or no Google embed URL).
 */
export const InteractiveMapView = memo(function InteractiveMapView({
  features,
  focusId,
}: {
  features: MapFeature[];
  focusId?: string;
}) {
  if (prefersGoogleEmbed(features)) {
    const focused =
      features.find((f) => f.id === focusId && googleMapsEmbedUrl(f)) ||
      features.find((f) => googleMapsEmbedUrl(f));
    if (focused) return <GoogleEmbedMap feature={focused} />;
  }
  return <LeafletMapView features={features} />;
});

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function escapeAttr(s: string): string {
  return escapeHtml(s).replace(/'/g, '&#39;');
}
