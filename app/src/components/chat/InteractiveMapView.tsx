import { useEffect, useRef, memo } from 'react';
import L from 'leaflet';
import 'leaflet/dist/leaflet.css';
import markerIconUrl from 'leaflet/dist/images/marker-icon.png';
import markerIcon2xUrl from 'leaflet/dist/images/marker-icon-2x.png';
import markerShadowUrl from 'leaflet/dist/images/marker-shadow.png';
import type { MapFeature } from './mapSources';

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

/**
 * Interactive OSM map for place markers and route polylines.
 * Uses Leaflet (no Google/Amap display key) so Tauri can render maps when the
 * user only configured MCP keys for search/routing.
 */
export const InteractiveMapView = memo(function InteractiveMapView({
  features,
}: {
  features: MapFeature[];
}) {
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

    // Leaflet needs a tick after layout to size correctly.
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
