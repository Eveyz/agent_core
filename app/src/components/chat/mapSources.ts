import type { TurnBlock } from '../../features/chat/types';

export type MapProvider = 'google' | 'amap' | 'unknown';

export type MapPlace = {
  kind: 'place';
  id: string;
  name: string;
  address?: string;
  lat?: number;
  lng?: number;
  /** Deep link opened when the card is clicked. */
  mapUrl: string;
  /** Optional static preview (no product API key). */
  previewUrl?: string;
  provider: MapProvider;
  toolName: string;
  callId: string;
};

export type MapRoute = {
  kind: 'route';
  id: string;
  title: string;
  summary?: string;
  distanceText?: string;
  durationText?: string;
  originLat?: number;
  originLng?: number;
  destLat?: number;
  destLng?: number;
  /** Optional path as [lat, lng] pairs for interactive map drawing. */
  polyline?: Array<[number, number]>;
  mapUrl: string;
  previewUrl?: string;
  provider: MapProvider;
  toolName: string;
  callId: string;
};

export type MapFeature = MapPlace | MapRoute;

export const FEATURED_MAP_LIMIT = 3;

const MAP_TOOL_RE =
  /(search_places|compute_routes|maps_(?:text_search|around_search|search_detail|geo|regeocode|direction_|bicycling|distance)|lookup_weather|resolve_names|resolve_maps_urls)/i;

export function isMapToolName(name: string): boolean {
  if (!name) return false;
  if (MAP_TOOL_RE.test(name)) return true;
  const lower = name.toLowerCase();
  return (
    lower.includes('amap') ||
    lower.includes('gaode') ||
    lower.includes('maps-grounding') ||
    lower.includes('mapstools') ||
    /__maps_/.test(lower)
  );
}

export function inferMapProvider(toolName: string): MapProvider {
  const lower = toolName.toLowerCase();
  if (lower.includes('amap') || lower.includes('gaode')) {
    return 'amap';
  }
  // Official Amap MCP tool names: maps_text_search, maps_direction_driving, …
  if (
    /(?:^|__)maps_(?:text_search|around_search|search_detail|geo|regeocode|direction_|bicycling|distance|weather|ip_location)/.test(
      lower,
    )
  ) {
    return 'amap';
  }
  if (
    lower.includes('search_places') ||
    lower.includes('compute_routes') ||
    lower.includes('lookup_weather') ||
    lower.includes('mapstools') ||
    lower.includes('grounding') ||
    lower.includes('google')
  ) {
    return 'google';
  }
  return 'unknown';
}

function isFiniteCoord(n: unknown): n is number {
  return typeof n === 'number' && Number.isFinite(n);
}

function asNumber(v: unknown): number | undefined {
  if (isFiniteCoord(v)) return v;
  if (typeof v === 'string' && v.trim()) {
    const n = Number(v);
    if (Number.isFinite(n)) return n;
  }
  return undefined;
}

/** Parse Amap `"lng,lat"` or Google-style objects. */
export function parseLatLng(value: unknown): { lat: number; lng: number } | null {
  if (!value) return null;

  if (typeof value === 'string') {
    const parts = value.split(',').map((p) => p.trim());
    if (parts.length >= 2) {
      const a = Number(parts[0]);
      const b = Number(parts[1]);
      if (!Number.isFinite(a) || !Number.isFinite(b)) return null;
      // Amap Web is lng,lat; Google strings are rarely used this way.
      // Heuristic: China lng is ~73–135, lat ~18–54 → if |a|>|b| and a in lng range, treat as lng,lat.
      if (Math.abs(a) > 90 || (Math.abs(a) > Math.abs(b) && Math.abs(a) <= 180)) {
        return { lng: a, lat: b };
      }
      return { lat: a, lng: b };
    }
    return null;
  }

  if (typeof value === 'object') {
    const row = value as Record<string, unknown>;
    const nested =
      (row.lat_lng && typeof row.lat_lng === 'object'
        ? (row.lat_lng as Record<string, unknown>)
        : null) ||
      (row.latLng && typeof row.latLng === 'object'
        ? (row.latLng as Record<string, unknown>)
        : null) ||
      (row.location && typeof row.location === 'object'
        ? (row.location as Record<string, unknown>)
        : null);
    const lat =
      asNumber(row.latitude) ??
      asNumber(row.lat) ??
      (nested
        ? (asNumber(nested.latitude) ?? asNumber(nested.lat))
        : undefined);
    const lng =
      asNumber(row.longitude) ??
      asNumber(row.lng) ??
      asNumber(row.lon) ??
      (nested
        ? (asNumber(nested.longitude) ?? asNumber(nested.lng) ?? asNumber(nested.lon))
        : undefined);
    if (lat !== undefined && lng !== undefined) return { lat, lng };
  }

  return null;
}

export function staticMapPreviewUrl(lat: number, lng: number): string {
  // Key-free OSM static preview. GCJ-02 (Amap) will be slightly offset on OSM — acceptable for MVP.
  const center = `${lat.toFixed(6)},${lng.toFixed(6)}`;
  return `https://staticmap.openstreetmap.de/staticmap.php?center=${center}&zoom=15&size=400x220&maptype=mapnik&markers=${center},red-pushpin`;
}

export function googlePlaceMapUrl(opts: {
  lat?: number;
  lng?: number;
  name?: string;
  placeUrl?: string;
}): string {
  if (opts.placeUrl && /^https?:\/\//i.test(opts.placeUrl)) return opts.placeUrl;
  if (opts.name?.trim()) {
    return `https://www.google.com/maps/search/?api=1&query=${encodeURIComponent(opts.name.trim())}`;
  }
  if (opts.lat !== undefined && opts.lng !== undefined) {
    return `https://www.google.com/maps/search/?api=1&query=${opts.lat},${opts.lng}`;
  }
  return 'https://www.google.com/maps';
}

export function amapPlaceMapUrl(opts: {
  lat?: number;
  lng?: number;
  name?: string;
}): string {
  if (opts.lat !== undefined && opts.lng !== undefined) {
    const name = encodeURIComponent(opts.name?.trim() || 'Place');
    return `https://uri.amap.com/marker?position=${opts.lng},${opts.lat}&name=${name}`;
  }
  if (opts.name?.trim()) {
    return `https://uri.amap.com/search?keyword=${encodeURIComponent(opts.name.trim())}`;
  }
  return 'https://www.amap.com';
}

export function googleDirectionsUrl(opts: {
  originLat?: number;
  originLng?: number;
  destLat?: number;
  destLng?: number;
  originName?: string;
  destName?: string;
}): string {
  const origin =
    opts.originLat !== undefined && opts.originLng !== undefined
      ? `${opts.originLat},${opts.originLng}`
      : opts.originName?.trim() || '';
  const destination =
    opts.destLat !== undefined && opts.destLng !== undefined
      ? `${opts.destLat},${opts.destLng}`
      : opts.destName?.trim() || '';
  if (!origin || !destination) return 'https://www.google.com/maps/dir/';
  return `https://www.google.com/maps/dir/?api=1&origin=${encodeURIComponent(origin)}&destination=${encodeURIComponent(destination)}`;
}

export function amapDirectionsUrl(opts: {
  originLat?: number;
  originLng?: number;
  destLat?: number;
  destLng?: number;
  originName?: string;
  destName?: string;
}): string {
  if (
    opts.originLat !== undefined &&
    opts.originLng !== undefined &&
    opts.destLat !== undefined &&
    opts.destLng !== undefined
  ) {
    const fromName = encodeURIComponent(opts.originName || 'Start');
    const toName = encodeURIComponent(opts.destName || 'End');
    return `https://uri.amap.com/navigation?from=${opts.originLng},${opts.originLat},${fromName}&to=${opts.destLng},${opts.destLat},${toName}&mode=car`;
  }
  return 'https://www.amap.com';
}

function placeMapUrl(
  provider: MapProvider,
  opts: { lat?: number; lng?: number; name?: string; placeUrl?: string },
): string {
  if (provider === 'amap') return amapPlaceMapUrl(opts);
  return googlePlaceMapUrl(opts);
}

function routeMapUrl(
  provider: MapProvider,
  opts: {
    originLat?: number;
    originLng?: number;
    destLat?: number;
    destLng?: number;
    originName?: string;
    destName?: string;
  },
): string {
  if (provider === 'amap') return amapDirectionsUrl(opts);
  return googleDirectionsUrl(opts);
}

function displayNameFromRow(row: Record<string, unknown>): string {
  const displayName = row.displayName;
  if (typeof displayName === 'string' && displayName.trim()) return displayName.trim();
  if (displayName && typeof displayName === 'object') {
    const text = (displayName as { text?: unknown }).text;
    if (typeof text === 'string' && text.trim()) return text.trim();
  }
  for (const key of ['name', 'title', 'place_name', 'poi_name'] as const) {
    const v = row[key];
    if (typeof v === 'string' && v.trim()) return v.trim();
  }
  // Google Maps Grounding Lite PlaceView uses attribution.title
  const attribution = row.attribution;
  if (attribution && typeof attribution === 'object') {
    const title = (attribution as { title?: unknown }).title;
    if (typeof title === 'string' && title.trim()) return title.trim();
  }
  return '';
}

function addressFromRow(row: Record<string, unknown>): string | undefined {
  for (const key of [
    'formattedAddress',
    'formatted_address',
    'address',
    'vicinity',
    'shortAddress',
  ] as const) {
    const v = row[key];
    if (typeof v === 'string' && v.trim()) return v.trim();
  }
  return undefined;
}

function googleMapsPlaceUrlFromRow(row: Record<string, unknown>): string | undefined {
  const links = row.googleMapsLinks;
  if (links && typeof links === 'object') {
    const placeUrl = (links as { placeUrl?: unknown }).placeUrl;
    if (typeof placeUrl === 'string' && /^https?:\/\//i.test(placeUrl)) return placeUrl;
  }
  for (const key of ['placeUrl', 'googleMapsUri', 'maps_url', 'url'] as const) {
    const v = row[key];
    if (typeof v === 'string' && /google\.[^/]+\/maps|maps\.app\.goo\.gl/i.test(v)) return v;
  }
  return undefined;
}

function coordsFromRow(row: Record<string, unknown>): { lat: number; lng: number } | null {
  return (
    parseLatLng(row.location) ||
    parseLatLng(row.latLng) ||
    parseLatLng(row.lat_lng) ||
    parseLatLng({
      latitude: row.latitude ?? row.lat,
      longitude: row.longitude ?? row.lng ?? row.lon,
    })
  );
}

function makePlace(
  row: Record<string, unknown>,
  toolName: string,
  callId: string,
  index: number,
  provider: MapProvider,
): MapPlace | null {
  const name = displayNameFromRow(row);
  const coords = coordsFromRow(row);
  const placeUrl = googleMapsPlaceUrlFromRow(row);
  if (!name && !coords && !placeUrl) return null;

  const label = name || addressFromRow(row) || (coords ? `${coords.lat.toFixed(4)}, ${coords.lng.toFixed(4)}` : 'Place');
  const mapUrl = placeMapUrl(provider, {
    lat: coords?.lat,
    lng: coords?.lng,
    name: label,
    placeUrl,
  });

  return {
    kind: 'place',
    id: `${callId}:place:${typeof row.id === 'string' ? row.id : index}`,
    name: label,
    address: addressFromRow(row),
    lat: coords?.lat,
    lng: coords?.lng,
    mapUrl,
    previewUrl: coords ? staticMapPreviewUrl(coords.lat, coords.lng) : undefined,
    provider,
    toolName,
    callId,
  };
}

function formatMeters(m: number): string {
  if (m >= 1000) return `${(m / 1000).toFixed(m >= 10000 ? 0 : 1)} km`;
  return `${Math.round(m)} m`;
}

function formatDurationSeconds(sec: number): string {
  if (sec < 60) return `${Math.round(sec)}s`;
  const mins = Math.round(sec / 60);
  if (mins < 60) return `${mins} min`;
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return m ? `${h} h ${m} min` : `${h} h`;
}

function distanceTextFromRow(row: Record<string, unknown>): string | undefined {
  const meters = asNumber(row.distanceMeters) ?? asNumber(row.distance);
  if (meters !== undefined) {
    // Amap distance is often meters as string; if > 1e6 treat as already formatted? unlikely.
    if (typeof row.distance === 'string' && /km|米|m/i.test(row.distance)) return row.distance;
    return formatMeters(meters);
  }
  if (typeof row.distance === 'string' && row.distance.trim()) return row.distance.trim();
  return undefined;
}

function durationTextFromRow(row: Record<string, unknown>): string | undefined {
  const duration = row.duration;
  if (typeof duration === 'string' && duration.trim()) {
    // Google often returns "1234s"
    const secMatch = duration.trim().match(/^(\d+(?:\.\d+)?)s$/i);
    if (secMatch) return formatDurationSeconds(Number(secMatch[1]));
    return duration.trim();
  }
  const sec =
    asNumber(row.duration_seconds) ??
    asNumber(row.durationSeconds) ??
    asNumber(row.duration);
  if (sec !== undefined) return formatDurationSeconds(sec);
  return undefined;
}

function parsePolyline(value: unknown): Array<[number, number]> | undefined {
  if (!value) return undefined;

  if (Array.isArray(value)) {
    const pts: Array<[number, number]> = [];
    for (const item of value) {
      const c = parseLatLng(item);
      if (c) pts.push([c.lat, c.lng]);
    }
    return pts.length >= 2 ? simplifyPolyline(pts, 80) : undefined;
  }

  if (typeof value === 'string' && value.includes(';')) {
    // Amap: "lng,lat;lng,lat;..."
    const pts: Array<[number, number]> = [];
    for (const part of value.split(';')) {
      const c = parseLatLng(part.trim());
      if (c) pts.push([c.lat, c.lng]);
    }
    return pts.length >= 2 ? simplifyPolyline(pts, 80) : undefined;
  }

  if (typeof value === 'object') {
    const row = value as Record<string, unknown>;
    return (
      parsePolyline(row.points) ||
      parsePolyline(row.coordinates) ||
      parsePolyline(row.encodedPolyline) || // string without ; — skip
      parsePolyline(row.polyline)
    );
  }

  return undefined;
}

function simplifyPolyline(
  pts: Array<[number, number]>,
  maxPoints: number,
): Array<[number, number]> {
  if (pts.length <= maxPoints) return pts;
  const out: Array<[number, number]> = [];
  const step = (pts.length - 1) / (maxPoints - 1);
  for (let i = 0; i < maxPoints; i++) {
    out.push(pts[Math.round(i * step)]);
  }
  return out;
}

function makeRoute(
  row: Record<string, unknown>,
  toolName: string,
  callId: string,
  index: number,
  provider: MapProvider,
  fallbackOrigin?: { lat: number; lng: number; name?: string },
  fallbackDest?: { lat: number; lng: number; name?: string },
): MapRoute | null {
  const origin =
    parseLatLng(row.origin) ||
    parseLatLng(row.start) ||
    parseLatLng(row.start_location) ||
    fallbackOrigin ||
    null;
  const dest =
    parseLatLng(row.destination) ||
    parseLatLng(row.end) ||
    parseLatLng(row.end_location) ||
    fallbackDest ||
    null;

  const originName =
    (typeof row.origin_name === 'string' && row.origin_name) ||
    (typeof row.originName === 'string' && row.originName) ||
    (typeof row.origin === 'object' &&
      row.origin &&
      typeof (row.origin as { address?: unknown }).address === 'string' &&
      (row.origin as { address: string }).address) ||
    fallbackOrigin?.name;
  const destName =
    (typeof row.destination_name === 'string' && row.destination_name) ||
    (typeof row.destinationName === 'string' && row.destinationName) ||
    (typeof row.destination === 'object' &&
      row.destination &&
      typeof (row.destination as { address?: unknown }).address === 'string' &&
      (row.destination as { address: string }).address) ||
    fallbackDest?.name;

  const distanceText = distanceTextFromRow(row);
  const durationText = durationTextFromRow(row);
  const polyline =
    parsePolyline(row.polyline) ||
    parsePolyline(row.steps) ||
    (origin && dest ? ([[origin.lat, origin.lng], [dest.lat, dest.lng]] as Array<[number, number]>) : undefined);

  // Need at least some route signal
  if (!origin && !dest && !distanceText && !durationText) return null;

  const titleParts = [originName || 'Start', destName || 'End'];
  const title = `${titleParts[0]} → ${titleParts[1]}`;
  const summaryParts = [distanceText, durationText].filter(Boolean);
  const midLat =
    origin && dest ? (origin.lat + dest.lat) / 2 : origin?.lat ?? dest?.lat;
  const midLng =
    origin && dest ? (origin.lng + dest.lng) / 2 : origin?.lng ?? dest?.lng;

  return {
    kind: 'route',
    id: `${callId}:route:${index}`,
    title,
    summary: summaryParts.length ? summaryParts.join(' · ') : undefined,
    distanceText,
    durationText,
    originLat: origin?.lat,
    originLng: origin?.lng,
    destLat: dest?.lat,
    destLng: dest?.lng,
    polyline,
    mapUrl: routeMapUrl(provider, {
      originLat: origin?.lat,
      originLng: origin?.lng,
      destLat: dest?.lat,
      destLng: dest?.lng,
      originName: typeof originName === 'string' ? originName : undefined,
      destName: typeof destName === 'string' ? destName : undefined,
    }),
    previewUrl:
      midLat !== undefined && midLng !== undefined
        ? staticMapPreviewUrl(midLat, midLng)
        : undefined,
    provider,
    toolName,
    callId,
  };
}

/** Pull the first JSON value from MCP text (may be wrapped in prose). */
export function extractJsonValue(result: string): unknown | null {
  const trimmed = result.trim();
  if (!trimmed) return null;

  const tryParse = (s: string): unknown | null => {
    try {
      return JSON.parse(s);
    } catch {
      return null;
    }
  };

  const direct = tryParse(trimmed);
  if (direct !== null) return direct;

  // Common: ```json ... ```
  const fence = trimmed.match(/```(?:json)?\s*([\s\S]*?)```/i);
  if (fence) {
    const fromFence = tryParse(fence[1].trim());
    if (fromFence !== null) return fromFence;
  }

  const startObj = trimmed.indexOf('{');
  const startArr = trimmed.indexOf('[');
  let start = -1;
  if (startObj >= 0 && startArr >= 0) start = Math.min(startObj, startArr);
  else start = Math.max(startObj, startArr);
  if (start < 0) return null;

  // Walk braces/brackets for a balanced slice
  const open = trimmed[start];
  const close = open === '{' ? '}' : ']';
  let depth = 0;
  let inString = false;
  let escape = false;
  for (let i = start; i < trimmed.length; i++) {
    const ch = trimmed[i];
    if (inString) {
      if (escape) escape = false;
      else if (ch === '\\') escape = true;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') {
      inString = true;
      continue;
    }
    if (ch === open) depth++;
    else if (ch === close) {
      depth--;
      if (depth === 0) {
        return tryParse(trimmed.slice(start, i + 1));
      }
    }
  }
  return null;
}

function collectPlaceArrays(root: unknown): unknown[] {
  if (!root || typeof root !== 'object') return [];
  if (Array.isArray(root)) return root;

  const obj = root as Record<string, unknown>;
  const keys = [
    'places',
    'pois',
    'results',
    'entities',
    'candidates',
    'geocodes',
  ] as const;
  for (const key of keys) {
    if (Array.isArray(obj[key])) return obj[key] as unknown[];
  }

  // Nested: { data: { places: [] } }
  for (const v of Object.values(obj)) {
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      const nested = collectPlaceArrays(v);
      if (nested.length) return nested;
    }
  }
  return [];
}

function collectRouteArrays(root: unknown): unknown[] {
  if (!root || typeof root !== 'object') return [];
  if (Array.isArray(root)) {
    // Heuristic: array of routes if items look like routes
    return root;
  }
  const obj = root as Record<string, unknown>;
  for (const key of ['routes', 'paths', 'route'] as const) {
    const v = obj[key];
    if (Array.isArray(v)) return v;
    if (v && typeof v === 'object') return [v];
  }
  for (const v of Object.values(obj)) {
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      const nested = collectRouteArrays(v);
      if (nested.length) return nested;
    }
  }
  return [];
}

function endpointFromRoot(
  root: Record<string, unknown>,
  which: 'origin' | 'destination',
): { lat: number; lng: number; name?: string } | undefined {
  const raw = root[which] ?? root[which === 'origin' ? 'start' : 'end'];
  const coords = parseLatLng(raw);
  let name: string | undefined;
  if (raw && typeof raw === 'object') {
    const row = raw as Record<string, unknown>;
    const n = displayNameFromRow(row);
    if (n) name = n;
    else if (typeof row.address === 'string') name = row.address;
  } else if (typeof raw === 'string' && raw.trim()) {
    name = raw.trim();
  }
  if (!coords) {
    // Address-only waypoint (Google compute_routes args often omit lat/lng)
    return name ? { lat: NaN, lng: NaN, name } : undefined;
  }
  return { ...coords, name };
}

function cleanEndpoint(
  ep?: { lat: number; lng: number; name?: string },
): { lat: number; lng: number; name?: string } | undefined {
  if (!ep) return undefined;
  if (!Number.isFinite(ep.lat) || !Number.isFinite(ep.lng)) {
    return ep.name ? { lat: NaN, lng: NaN, name: ep.name } : undefined;
  }
  return ep;
}

export function parseMapFeaturesFromJson(
  result: string,
  toolName: string,
  callId: string,
  toolArgs?: unknown,
): MapFeature[] {
  const parsed = extractJsonValue(result);
  if (parsed === null && !toolArgs) return [];

  const provider = inferMapProvider(toolName);
  const out: MapFeature[] = [];
  const lower = toolName.toLowerCase();
  const preferRoutes =
    lower.includes('compute_routes') ||
    lower.includes('direction') ||
    lower.includes('bicycling') ||
    lower.includes('distance');

  const rootObj =
    parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : null;
  const argsObj =
    toolArgs && typeof toolArgs === 'object' && !Array.isArray(toolArgs)
      ? (toolArgs as Record<string, unknown>)
      : typeof toolArgs === 'string'
        ? (() => {
            try {
              const v = JSON.parse(toolArgs);
              return v && typeof v === 'object' && !Array.isArray(v)
                ? (v as Record<string, unknown>)
                : null;
            } catch {
              return null;
            }
          })()
        : null;

  if (preferRoutes || (rootObj && (rootObj.routes || rootObj.paths)) || (parsed === null && argsObj)) {
    const routes = parsed !== null ? collectRouteArrays(parsed) : [];
    const origin =
      cleanEndpoint(rootObj ? endpointFromRoot(rootObj, 'origin') : undefined) ||
      cleanEndpoint(argsObj ? endpointFromRoot(argsObj, 'origin') : undefined);
    const dest =
      cleanEndpoint(rootObj ? endpointFromRoot(rootObj, 'destination') : undefined) ||
      cleanEndpoint(argsObj ? endpointFromRoot(argsObj, 'destination') : undefined);

    const originFb = origin && Number.isFinite(origin.lat) ? origin : undefined;
    const destFb = dest && Number.isFinite(dest.lat) ? dest : undefined;
    const originNameOnly = origin && !Number.isFinite(origin.lat) ? origin.name : undefined;
    const destNameOnly = dest && !Number.isFinite(dest.lat) ? dest.name : undefined;

    routes.forEach((item, i) => {
      if (!item || typeof item !== 'object') return;
      const row = { ...(item as Record<string, unknown>) };
      if (originNameOnly && !row.origin_name) row.origin_name = originNameOnly;
      if (destNameOnly && !row.destination_name) row.destination_name = destNameOnly;
      const route = makeRoute(row, toolName, callId, i, provider, originFb, destFb);
      if (route) out.push(route);
    });
    // Single-object route response (distance/duration at root) — Google Grounding Lite shape
    if (out.length === 0 && (rootObj || argsObj)) {
      const row = { ...(rootObj || {}) };
      if (originNameOnly) row.origin_name = originNameOnly;
      if (destNameOnly) row.destination_name = destNameOnly;
      if (argsObj?.travelMode && !row.travelMode) row.travelMode = argsObj.travelMode;
      const route = makeRoute(row, toolName, callId, 0, provider, originFb, destFb);
      if (route) out.push(route);
    }
  }

  if (parsed !== null) {
    const places = collectPlaceArrays(parsed);
    places.forEach((item, i) => {
      if (!item || typeof item !== 'object') return;
      const place = makePlace(item as Record<string, unknown>, toolName, callId, i, provider);
      if (place) out.push(place);
    });

    // Single place object
    if (out.length === 0 && rootObj && (rootObj.location || rootObj.latitude || rootObj.name || rootObj.attribution)) {
      const place = makePlace(rootObj, toolName, callId, 0, provider);
      if (place) out.push(place);
    }
  }

  return out;
}

/** Compact map MCP JSON so the 5000-char UI truncate keeps places/routes parseable. */
export function compactMapToolResult(toolName: string, result: string): string {
  if (!isMapToolName(toolName) || !result) return result;
  const parsed = extractJsonValue(result);
  if (parsed === null || typeof parsed !== 'object') return result;

  const compactPlace = (item: unknown): Record<string, unknown> | null => {
    if (!item || typeof item !== 'object') return null;
    const row = item as Record<string, unknown>;
    const coords = coordsFromRow(row);
    const name = displayNameFromRow(row);
    const address = addressFromRow(row);
    const placeUrl = googleMapsPlaceUrlFromRow(row);
    const out: Record<string, unknown> = {};
    if (typeof row.id === 'string') out.id = row.id;
    if (name) out.name = name;
    if (address) out.address = address;
    if (coords) out.location = { latitude: coords.lat, longitude: coords.lng };
    if (placeUrl) out.googleMapsLinks = { placeUrl };
    if (row.attribution && typeof row.attribution === 'object') {
      const title = (row.attribution as { title?: unknown }).title;
      if (typeof title === 'string') out.attribution = { title };
    }
    return Object.keys(out).length ? out : null;
  };

  const compactRoute = (item: unknown): Record<string, unknown> | null => {
    if (!item || typeof item !== 'object') return null;
    const row = item as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    if (row.distanceMeters !== undefined) out.distanceMeters = row.distanceMeters;
    else if (row.distance !== undefined) out.distance = row.distance;
    if (row.duration !== undefined) out.duration = row.duration;
    const origin = parseLatLng(row.origin) || parseLatLng(row.start);
    const dest = parseLatLng(row.destination) || parseLatLng(row.end);
    if (origin) out.origin = { latitude: origin.lat, longitude: origin.lng };
    if (dest) out.destination = { latitude: dest.lat, longitude: dest.lng };
    const poly = parsePolyline(row.polyline) || parsePolyline(row.polyline);
    if (poly) {
      // Store as Amap-style compact string lng,lat;...
      out.polyline = poly.map(([lat, lng]) => `${lng},${lat}`).join(';');
    }
    return Object.keys(out).length ? out : null;
  };

  const root = parsed as Record<string, unknown>;
  const compacted: Record<string, unknown> = {};

  if (typeof root.summary === 'string') {
    compacted.summary = root.summary.slice(0, 400);
  }

  const places = collectPlaceArrays(parsed)
    .map(compactPlace)
    .filter((p): p is Record<string, unknown> => !!p)
    .slice(0, 12);
  if (places.length) {
    if (Array.isArray(root.pois)) compacted.pois = places;
    else compacted.places = places;
  }

  const routes = collectRouteArrays(parsed)
    .map(compactRoute)
    .filter((r): r is Record<string, unknown> => !!r)
    .slice(0, 3);
  if (routes.length) compacted.routes = routes;

  // Preserve top-level origin/destination when present (route helper)
  for (const key of ['origin', 'destination'] as const) {
    const c = parseLatLng(root[key]);
    if (c) compacted[key] = { latitude: c.lat, longitude: c.lng };
    else if (root[key] && typeof root[key] === 'object') {
      const addr = (root[key] as { address?: unknown }).address;
      if (typeof addr === 'string') compacted[key] = { address: addr };
    }
  }

  if (Object.keys(compacted).length === 0) return result;

  try {
    return JSON.stringify(compacted);
  } catch {
    return result;
  }
}

export function dedupeMapFeatures(features: MapFeature[]): MapFeature[] {
  const seen = new Set<string>();
  const out: MapFeature[] = [];
  for (const f of features) {
    const key =
      f.kind === 'place'
        ? `p:${(f.lat ?? '').toString()},${(f.lng ?? '').toString()}:${f.name.toLowerCase()}:${f.mapUrl}`
        : `r:${f.title.toLowerCase()}:${f.summary || ''}:${f.mapUrl}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(f);
  }
  return out;
}

export function extractMapFeaturesFromBlocks(blocks: TurnBlock[]): MapFeature[] {
  const collected: MapFeature[] = [];
  for (const block of blocks) {
    if (block.type !== 'tool' || block.is_error || !block.result) continue;
    if (!isMapToolName(block.name)) {
      // Still accept obvious place JSON from map-ish generic results when coords present
      const maybe = parseMapFeaturesFromJson(block.result, block.name, block.call_id, block.args);
      const hasCoords = maybe.some(
        (f) =>
          (f.kind === 'place' && f.lat !== undefined && f.lng !== undefined) ||
          (f.kind === 'route' &&
            ((f.originLat !== undefined && f.originLng !== undefined) ||
              (f.destLat !== undefined && f.destLng !== undefined) ||
              !!f.summary)),
      );
      if (!hasCoords && !maybe.some((f) => f.kind === 'route')) continue;
      // Avoid stealing web search results: require place-like keys
      const parsed = extractJsonValue(block.result);
      if (
        !parsed ||
        typeof parsed !== 'object' ||
        Array.isArray(parsed) ||
        !(
          'places' in parsed ||
          'pois' in parsed ||
          'routes' in parsed ||
          'paths' in parsed ||
          'geocodes' in parsed
        )
      ) {
        continue;
      }
      collected.push(...maybe);
      continue;
    }
    collected.push(
      ...parseMapFeaturesFromJson(block.result, block.name, block.call_id, block.args),
    );
  }
  return dedupeMapFeatures(collected);
}

export function featuredMapFeatures(
  features: MapFeature[],
  limit: number = FEATURED_MAP_LIMIT,
): MapFeature[] {
  return features.slice(0, Math.max(0, limit));
}

export function extractMapFeaturesFromEntries(
  entries: Array<{ type?: string; blocks?: TurnBlock[] }>,
): MapFeature[] {
  const collected: MapFeature[] = [];
  for (const entry of entries) {
    if (entry.type !== 'turn' || !entry.blocks) continue;
    collected.push(...extractMapFeaturesFromBlocks(entry.blocks));
  }
  return dedupeMapFeatures(collected);
}

export function providerLabel(provider: MapProvider): string {
  if (provider === 'amap') return 'Amap';
  if (provider === 'google') return 'Google Maps';
  return 'Maps';
}
