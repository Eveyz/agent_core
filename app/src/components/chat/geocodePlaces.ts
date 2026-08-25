import { staticMapPreviewUrl, type MapFeature, type MapPlace } from './mapSources';

export type LatLng = { lat: number; lng: number };

const cache = new Map<string, LatLng | null>();

function cacheKey(query: string): string {
  return query.trim().toLowerCase();
}

function parsePhoton(data: unknown): LatLng | null {
  if (!data || typeof data !== 'object') return null;
  const features = (data as { features?: unknown }).features;
  if (!Array.isArray(features) || features.length === 0) return null;
  const geometry = (features[0] as { geometry?: { coordinates?: unknown } })?.geometry;
  const coords = geometry?.coordinates;
  if (!Array.isArray(coords) || coords.length < 2) return null;
  const lng = Number(coords[0]);
  const lat = Number(coords[1]);
  if (!Number.isFinite(lat) || !Number.isFinite(lng)) return null;
  return { lat, lng };
}

/** Resolve a place name/address to coordinates for map display (no Google display key). */
export async function geocodeQuery(query: string): Promise<LatLng | null> {
  const key = cacheKey(query);
  if (!key) return null;
  if (cache.has(key)) return cache.get(key) ?? null;

  let result: LatLng | null = null;
  try {
    const photonUrl = `https://photon.komoot.io/api/?limit=1&q=${encodeURIComponent(query.trim())}`;
    const photonRes = await fetch(photonUrl, { headers: { Accept: 'application/json' } });
    if (photonRes.ok) {
      result = parsePhoton(await photonRes.json());
    }
  } catch {
    result = null;
  }

  cache.set(key, result);
  return result;
}

function placeNeedsGeocode(place: MapPlace): boolean {
  return place.lat === undefined || place.lng === undefined;
}

function geocodeTextForPlace(place: MapPlace): string | undefined {
  const name = place.name?.trim();
  if (name && !/^Place\s/i.test(name)) return name;
  const address = place.address?.trim();
  if (address && !/confidence/i.test(address)) return address;
  return name || address || undefined;
}

/**
 * Fill missing lat/lng on places so Leaflet can draw pins.
 * Uses OSM Photon (+ Nominatim fallback). Display-only; Place ID deep links stay Google.
 */
export async function enrichMapFeaturesWithGeocode(
  features: MapFeature[],
): Promise<MapFeature[]> {
  const pending = features.filter(
    (f): f is MapPlace => f.kind === 'place' && placeNeedsGeocode(f) && !!geocodeTextForPlace(f),
  );
  if (pending.length === 0) return features;

  const coordsById = new Map<string, LatLng>();
  for (const place of pending) {
    const query = geocodeTextForPlace(place);
    if (!query) continue;
    const coords = await geocodeQuery(query);
    if (coords) coordsById.set(place.id, coords);
  }

  if (coordsById.size === 0) return features;

  return features.map((f) => {
    if (f.kind !== 'place') return f;
    const coords = coordsById.get(f.id);
    if (!coords) return f;
    return {
      ...f,
      lat: coords.lat,
      lng: coords.lng,
      previewUrl: f.previewUrl || staticMapPreviewUrl(coords.lat, coords.lng),
    };
  });
}

/** Test helper — clear geocode cache between cases. */
export function clearGeocodeCache(): void {
  cache.clear();
}
