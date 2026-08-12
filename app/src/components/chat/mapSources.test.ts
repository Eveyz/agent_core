import { describe, expect, it } from 'vitest';
import type { TurnBlock } from '../../features/chat/types';
import {
  FEATURED_MAP_LIMIT,
  compactMapToolResult,
  dedupeMapFeatures,
  extractJsonValue,
  extractMapFeaturesFromBlocks,
  extractMapFeaturesFromEntries,
  featuredMapFeatures,
  googleDirectionsUrl,
  googlePlaceMapUrl,
  isMapToolName,
  parseLatLng,
  parseMapFeaturesFromJson,
  staticMapPreviewUrl,
} from './mapSources';

const GOOGLE_PLACES = JSON.stringify({
  summary: 'Coffee near the park.',
  places: [
    {
      id: 'ChIJ1',
      displayName: { text: 'Blue Bottle Coffee' },
      formattedAddress: '66 Mint St, San Francisco, CA',
      location: { latitude: 37.7825, longitude: -122.4077 },
      googleMapsLinks: {
        placeUrl: 'https://maps.google.com/?cid=1',
      },
    },
    {
      id: 'ChIJ2',
      name: 'Sightglass',
      location: { lat: 37.7769, lng: -122.4085 },
    },
    {
      id: 'ChIJ3',
      name: 'Third',
      location: { latitude: 37.77, longitude: -122.41 },
    },
    {
      id: 'ChIJ4',
      name: 'Fourth',
      location: { latitude: 37.78, longitude: -122.42 },
    },
  ],
});

const AMAP_POIS = JSON.stringify({
  pois: [
    {
      id: 'B000A8UR46',
      name: '天安门',
      address: '东城区东长安街',
      location: '116.397428,39.90923',
    },
  ],
});

const GOOGLE_ROUTE = JSON.stringify({
  routes: [
    {
      distanceMeters: 54000,
      duration: '2400s',
      attribution: { title: 'Google Maps', url: 'https://maps.google.com/' },
    },
  ],
});

describe('isMapToolName', () => {
  it('matches google and amap tool names', () => {
    expect(isMapToolName('mcp__maps__search_places')).toBe(true);
    expect(isMapToolName('mcp__amap__maps_text_search')).toBe(true);
    expect(isMapToolName('mcp__x__compute_routes')).toBe(true);
    expect(isMapToolName('bash')).toBe(false);
    expect(isMapToolName('mcp__parallel__web_search')).toBe(false);
  });
});

describe('parseLatLng', () => {
  it('parses google objects and amap lng,lat strings', () => {
    expect(parseLatLng({ latitude: 1.2, longitude: 3.4 })).toEqual({ lat: 1.2, lng: 3.4 });
    expect(parseLatLng('116.397428,39.90923')).toEqual({
      lng: 116.397428,
      lat: 39.90923,
    });
  });
});

describe('extractJsonValue', () => {
  it('unwraps fenced json', () => {
    const v = extractJsonValue('Here:\n```json\n{"a":1}\n```\n');
    expect(v).toEqual({ a: 1 });
  });
});

describe('parseMapFeaturesFromJson', () => {
  it('parses Google search_places', () => {
    const features = parseMapFeaturesFromJson(
      GOOGLE_PLACES,
      'mcp__maps-grounding-lite__search_places',
      'c1',
    );
    expect(features.filter((f) => f.kind === 'place')).toHaveLength(4);
    const first = features[0];
    expect(first.kind).toBe('place');
    if (first.kind === 'place') {
      expect(first.name).toBe('Blue Bottle Coffee');
      expect(first.lat).toBeCloseTo(37.7825);
      expect(first.mapUrl).toContain('maps.google.com');
      expect(first.previewUrl).toContain('staticmap.openstreetmap.de');
      expect(first.provider).toBe('google');
    }
  });

  it('parses official Grounding Lite PlaceView (attribution.title)', () => {
    const payload = JSON.stringify({
      summary: 'Try [0] for coffee.',
      places: [
        {
          place: 'places/ChIJtest',
          id: 'ChIJtest',
          location: { latitude: 37.422, longitude: -122.084 },
          googleMapsLinks: {
            placeUrl: 'https://maps.google.com/?cid=99',
          },
          attribution: { title: 'Googleplex Cafe', url: 'https://maps.google.com/?cid=99' },
        },
      ],
    });
    const features = parseMapFeaturesFromJson(
      payload,
      'mcp__maps-grounding-lite__search_places',
      'g1',
    );
    expect(features).toHaveLength(1);
    expect(features[0].kind).toBe('place');
    if (features[0].kind === 'place') {
      expect(features[0].name).toBe('Googleplex Cafe');
    }
  });

  it('ignores branding attribution and uses summary bold name', () => {
    const payload = JSON.stringify({
      summary:
        '**6-8 Mercury Street** in Tin Hau, Hong Kong, is currently operational during its open hours [0].',
      places: [
        {
          id: 'ChIJBUVgzAEBBDQRBpw9Bk7GsXc',
          name: '- Google Maps',
          googleMapsLinks: {
            placeUrl:
              'https://www.google.com/maps/place//data=!4m2!3m1!1s0x34040101cc604505:0x77b1c64e063d9c06',
          },
          attribution: { title: ' - Google Maps' },
        },
      ],
    });
    const features = parseMapFeaturesFromJson(
      payload,
      'mcp__google-map__search_places',
      'g2',
      { textQuery: '6-8 Mercury Street Tin Hau Hong Kong' },
    );
    expect(features).toHaveLength(1);
    expect(features[0].kind).toBe('place');
    if (features[0].kind === 'place') {
      expect(features[0].name).toBe('6-8 Mercury Street');
      expect(features[0].mapUrl).toContain('google.com/maps');
      expect(features[0].lat).toBeUndefined();
    }
  });

  it('parses Amap pois', () => {
    const features = parseMapFeaturesFromJson(
      AMAP_POIS,
      'mcp__amap-maps__maps_text_search',
      'a1',
    );
    expect(features).toHaveLength(1);
    const place = features[0];
    expect(place.kind).toBe('place');
    if (place.kind === 'place') {
      expect(place.name).toBe('天安门');
      expect(place.lng).toBeCloseTo(116.397428);
      expect(place.lat).toBeCloseTo(39.90923);
      expect(place.mapUrl).toContain('uri.amap.com');
      expect(place.provider).toBe('amap');
    }
  });

  it('parses Google compute_routes with args endpoints', () => {
    const features = parseMapFeaturesFromJson(
      GOOGLE_ROUTE,
      'mcp__maps__compute_routes',
      'r1',
      {
        origin: { address: 'Googleplex', latLng: { latitude: 37.422, longitude: -122.084 } },
        destination: { address: 'SFO', latLng: { latitude: 37.6213, longitude: -122.379 } },
        travelMode: 'DRIVE',
      },
    );
    const routes = features.filter((f) => f.kind === 'route');
    expect(routes.length).toBeGreaterThanOrEqual(1);
    const route = routes[0];
    if (route.kind === 'route') {
      expect(route.distanceText).toContain('km');
      expect(route.durationText).toContain('min');
      expect(route.mapUrl).toContain('google.com/maps/dir');
      expect(route.polyline?.length).toBeGreaterThanOrEqual(2);
    }
  });
});

describe('extractMapFeaturesFromBlocks', () => {
  it('extracts map tools and skips bash', () => {
    const blocks: TurnBlock[] = [
      {
        type: 'tool',
        call_id: '1',
        name: 'mcp__maps__search_places',
        result: GOOGLE_PLACES,
        active: false,
        is_error: false,
      },
      {
        type: 'tool',
        call_id: '2',
        name: 'bash',
        result: GOOGLE_PLACES,
        active: false,
        is_error: false,
      },
    ];
    const features = extractMapFeaturesFromBlocks(blocks);
    expect(features.filter((f) => f.kind === 'place')).toHaveLength(4);
  });

  it('skips error tools', () => {
    const blocks: TurnBlock[] = [
      {
        type: 'tool',
        call_id: 'e',
        name: 'mcp__maps__search_places',
        result: GOOGLE_PLACES,
        active: false,
        is_error: true,
      },
    ];
    expect(extractMapFeaturesFromBlocks(blocks)).toEqual([]);
  });
});

describe('featuredMapFeatures', () => {
  it('limits to featured count', () => {
    const features = parseMapFeaturesFromJson(
      GOOGLE_PLACES,
      'mcp__maps__search_places',
      'c',
    );
    expect(featuredMapFeatures(features)).toHaveLength(FEATURED_MAP_LIMIT);
  });
});

describe('extractMapFeaturesFromEntries', () => {
  it('merges session turns', () => {
    const entries = [
      {
        type: 'turn',
        blocks: [
          {
            type: 'tool' as const,
            call_id: '1',
            name: 'mcp__amap__maps_text_search',
            result: AMAP_POIS,
            active: false,
            is_error: false,
          },
        ],
      },
      {
        type: 'turn',
        blocks: [
          {
            type: 'tool' as const,
            call_id: '2',
            name: 'mcp__maps__search_places',
            result: JSON.stringify({
              places: [
                {
                  name: 'Ferry Building',
                  location: { latitude: 37.7955, longitude: -122.3937 },
                },
              ],
            }),
            active: false,
            is_error: false,
          },
        ],
      },
    ];
    const features = extractMapFeaturesFromEntries(entries);
    expect(features.length).toBe(2);
  });
});

describe('url helpers', () => {
  it('builds preview and map urls', () => {
    expect(staticMapPreviewUrl(1, 2)).toContain('1.000000,2.000000');
    expect(googlePlaceMapUrl({ name: 'SF MoMA' })).toContain('SF%20MoMA');
    expect(
      googleDirectionsUrl({
        originLat: 1,
        originLng: 2,
        destLat: 3,
        destLng: 4,
      }),
    ).toContain('origin=1%2C2');
  });
});

describe('dedupeMapFeatures', () => {
  it('keeps first place', () => {
    const a = parseMapFeaturesFromJson(AMAP_POIS, 'mcp__amap__maps_text_search', '1');
    const b = parseMapFeaturesFromJson(AMAP_POIS, 'mcp__amap__maps_text_search', '2');
    expect(dedupeMapFeatures([...a, ...b])).toHaveLength(1);
  });
});

describe('compactMapToolResult', () => {
  it('shrinks large place payloads under the UI truncate budget', () => {
    const places = Array.from({ length: 40 }, (_, i) => ({
      id: `id-${i}`,
      attribution: { title: `Place ${i} with a rather long descriptive name for testing` },
      location: { latitude: 37.4 + i * 0.01, longitude: -122.1 - i * 0.01 },
      googleMapsLinks: {
        placeUrl: `https://maps.google.com/?cid=${i}`,
        directionsUrl: `https://maps.google.com/dir/${i}`,
        photosUrl: `https://maps.google.com/photos/${i}`,
        reviewsUrl: `https://maps.google.com/reviews/${i}`,
        writeAReviewUrl: `https://maps.google.com/write/${i}`,
      },
      fluff: 'x'.repeat(200),
    }));
    const fat = JSON.stringify({ summary: 'Lots of places. '.repeat(50), places }, null, 2);
    expect(fat.length).toBeGreaterThan(5000);
    const compact = compactMapToolResult('mcp__maps__search_places', fat);
    expect(compact.length).toBeLessThan(5000);
    const features = parseMapFeaturesFromJson(compact, 'mcp__maps__search_places', 'c');
    expect(features.length).toBeGreaterThan(0);
    expect(features[0].kind).toBe('place');
  });
});
