import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import {
  clearGeocodeCache,
  enrichMapFeaturesWithGeocode,
  geocodeQuery,
} from './geocodePlaces';
import type { MapPlace } from './mapSources';

function place(partial: Partial<MapPlace> & Pick<MapPlace, 'id' | 'name'>): MapPlace {
  return {
    kind: 'place',
    mapUrl: 'https://maps.google.com/?q=test',
    provider: 'google',
    toolName: 'mcp__google-map__resolve_names',
    callId: 'rn1',
    ...partial,
  };
}

describe('geocodePlaces', () => {
  beforeEach(() => {
    clearGeocodeCache();
    vi.stubGlobal(
      'fetch',
      vi.fn(async (input: RequestInfo | URL) => {
        const url = String(input);
        if (url.includes('photon.komoot.io')) {
          return {
            ok: true,
            json: async () => ({
              features: [
                { geometry: { coordinates: [114.1925, 22.2824] } },
              ],
            }),
          };
        }
        return { ok: false, json: async () => [] };
      }),
    );
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    clearGeocodeCache();
  });

  it('parses Photon GeoJSON coordinates', async () => {
    const coords = await geocodeQuery('Tin Hau MTR Station Hong Kong');
    expect(coords).toEqual({ lat: 22.2824, lng: 114.1925 });
  });

  it('fills missing lat/lng on place cards', async () => {
    const features = await enrichMapFeaturesWithGeocode([
      place({ id: 'p1', name: 'Tin Hau MTR Station Hong Kong' }),
    ]);
    expect(features[0].kind).toBe('place');
    if (features[0].kind === 'place') {
      expect(features[0].lat).toBe(22.2824);
      expect(features[0].lng).toBe(114.1925);
      expect(features[0].previewUrl).toContain('22.282400');
    }
  });

  it('leaves places with coordinates unchanged', async () => {
    const features = await enrichMapFeaturesWithGeocode([
      place({ id: 'p1', name: 'Already pinned', lat: 1, lng: 2 }),
    ]);
    expect(fetch).not.toHaveBeenCalled();
    if (features[0].kind === 'place') {
      expect(features[0].lat).toBe(1);
      expect(features[0].lng).toBe(2);
    }
  });
});
