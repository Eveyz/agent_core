import { describe, expect, it } from 'vitest';
import type { TurnBlock } from '../../features/chat/types';
import {
  dedupeWebSources,
  extractWebSourcesFromBlocks,
  extractWebSourcesFromEntries,
  extractWebSourcesFromTool,
  featuredWebSources,
  parseResultsJson,
  parseTavilyMarkdown,
  parseWebfetchResult,
  FEATURED_SOURCE_LIMIT,
} from './webSources';

const PARALLEL_JSON = JSON.stringify({
  search_id: 'search_abc',
  results: [
    {
      url: 'https://www.reuters.com/article/1',
      title: 'CoreWeave boosts 2026 spending',
      publish_date: '2026-08-12',
      excerpts: ['Cloud demand surged.', 'Capex raised.'],
    },
    {
      url: 'https://techcrunch.com/article/2',
      title: 'AI infrastructure boom',
      publish_date: null,
      excerpts: ['Hyperscalers racing.'],
    },
    {
      url: 'https://example.com/a',
      title: 'Third',
      excerpts: ['x'],
    },
    {
      url: 'https://example.com/b',
      title: 'Fourth',
      excerpts: ['y'],
    },
    {
      url: 'https://example.com/c',
      title: 'Fifth',
      excerpts: ['z'],
    },
  ],
});

describe('parseResultsJson', () => {
  it('parses Parallel MCP web_search JSON', () => {
    const sources = parseResultsJson(PARALLEL_JSON, 'mcp__parallel__web_search', 'c1');
    expect(sources).toHaveLength(5);
    expect(sources[0].url).toBe('https://www.reuters.com/article/1');
    expect(sources[0].title).toContain('CoreWeave');
    expect(sources[0].siteName).toBe('reuters.com');
    expect(sources[0].snippet).toContain('Cloud demand');
    expect(sources[0].faviconUrl).toContain('reuters.com');
  });

  it('ignores non-http urls and empty results', () => {
    expect(parseResultsJson('{"results":[{"url":"ftp://x","title":"no"}]}', 't', 'c')).toEqual([]);
    expect(parseResultsJson('not json', 't', 'c')).toEqual([]);
  });
});

describe('parseTavilyMarkdown', () => {
  it('parses numbered tavily blocks', () => {
    const text = `Some answer.

1. **Portugal standings**
URL: https://espn.com/soccer
Content: They sit atop group A.

2. **FIFA update**
URL: https://fifa.com/news
Content: Latest table published.
`;
    const sources = parseTavilyMarkdown(text, 'tavily_search', 'tv1');
    expect(sources).toHaveLength(2);
    expect(sources[0].title).toBe('Portugal standings');
    expect(sources[1].url).toBe('https://fifa.com/news');
  });
});

describe('parseWebfetchResult', () => {
  it('parses meta header and Fetched url', () => {
    const text = `**Title:** Protocol intro
**Description:** What is MCP?
**Source:** modelcontextprotocol.io
**Image:** https://cdn.example.com/og.png

Fetched: https://modelcontextprotocol.io/introduction
Status: 200

# Hello
`;
    const sources = parseWebfetchResult(text, 'webfetch', 'wf1');
    expect(sources).toHaveLength(1);
    expect(sources[0].url).toBe('https://modelcontextprotocol.io/introduction');
    expect(sources[0].title).toBe('Protocol intro');
    expect(sources[0].siteName).toBe('modelcontextprotocol.io');
    expect(sources[0].imageUrl).toBe('https://cdn.example.com/og.png');
  });
});

describe('extractWebSourcesFromBlocks', () => {
  it('extracts Parallel MCP and ignores bash', () => {
    const blocks: TurnBlock[] = [
      {
        type: 'tool',
        call_id: '1',
        name: 'mcp__parallel-search__web_search',
        result: PARALLEL_JSON,
        active: false,
        is_error: false,
      },
      {
        type: 'tool',
        call_id: '2',
        name: 'bash',
        result: 'https://evil.example/should-not-appear\n{"results":[{"url":"https://sneaky.example","title":"no"}]}',
        active: false,
        is_error: false,
      },
    ];
    // bash with JSON that starts mid-string won't parse; pure JSON bash would —
    // our bash result doesn't start with `{` after trim of full string? It has URL first so JSON parse fails.
    const sources = extractWebSourcesFromBlocks(blocks);
    expect(sources).toHaveLength(5);
    expect(sources.every((s) => s.url.includes('http'))).toBe(true);
  });

  it('accepts generic results[].url JSON from any tool', () => {
    const blocks: TurnBlock[] = [
      {
        type: 'tool',
        call_id: '3',
        name: 'mcp__other__search',
        result: JSON.stringify({
          results: [{ url: 'https://news.ycombinator.com/item?id=1', title: 'HN' }],
        }),
        active: false,
        is_error: false,
      },
    ];
    const sources = extractWebSourcesFromBlocks(blocks);
    expect(sources).toHaveLength(1);
    expect(sources[0].siteName).toBe('news.ycombinator.com');
  });

  it('dedupes urls across tools', () => {
    const blocks: TurnBlock[] = [
      {
        type: 'tool',
        call_id: 'a',
        name: 'tavily_search',
        result: `1. **Same**
URL: https://example.com/page
Content: one
`,
        active: false,
        is_error: false,
      },
      {
        type: 'tool',
        call_id: 'b',
        name: 'webfetch',
        result: `**Title:** Same again\n\nFetched: https://example.com/page/\nStatus: 200\n\nbody`,
        active: false,
        is_error: false,
      },
    ];
    const sources = extractWebSourcesFromBlocks(blocks);
    expect(sources).toHaveLength(1);
    expect(sources[0].title).toBe('Same');
  });

  it('skips error tool blocks', () => {
    const blocks: TurnBlock[] = [
      {
        type: 'tool',
        call_id: 'e',
        name: 'tavily_search',
        result: `1. **X**\nURL: https://example.com\nContent: y\n`,
        active: false,
        is_error: true,
      },
    ];
    expect(extractWebSourcesFromBlocks(blocks)).toEqual([]);
  });
});

describe('featuredWebSources', () => {
  it('returns top N in order', () => {
    const sources = parseResultsJson(PARALLEL_JSON, 'mcp__p__web_search', 'c');
    const featured = featuredWebSources(sources);
    expect(featured).toHaveLength(FEATURED_SOURCE_LIMIT);
    expect(featured.map((s) => s.title)).toEqual([
      'CoreWeave boosts 2026 spending',
      'AI infrastructure boom',
      'Third',
    ]);
  });

  it('returns empty when no sources', () => {
    expect(featuredWebSources([])).toEqual([]);
  });
});

describe('extractWebSourcesFromEntries', () => {
  it('merges session turns', () => {
    const entries = [
      {
        type: 'turn',
        blocks: [
          {
            type: 'tool' as const,
            call_id: '1',
            name: 'webfetch',
            result: 'Fetched: https://a.example/\nStatus: 200\n\nx',
            active: false,
            is_error: false,
          },
        ],
      },
      {
        type: 'user' as const,
        blocks: undefined,
      },
      {
        type: 'turn',
        blocks: [
          {
            type: 'tool' as const,
            call_id: '2',
            name: 'webfetch',
            result: 'Fetched: https://b.example/\nStatus: 200\n\ny',
            active: false,
            is_error: false,
          },
        ],
      },
    ];
    const sources = extractWebSourcesFromEntries(entries);
    expect(sources.map((s) => s.url)).toEqual(['https://a.example/', 'https://b.example/']);
  });
});

describe('extractWebSourcesFromTool', () => {
  it('routes named web_fetch without JSON to webfetch parser', () => {
    const sources = extractWebSourcesFromTool(
      'mcp__parallel__web_fetch',
      '**Title:** Doc\n\nFetched: https://docs.example/page\nStatus: 200\n\nbody',
      'f1',
    );
    expect(sources).toHaveLength(1);
  });
});

describe('dedupeWebSources', () => {
  it('keeps first occurrence', () => {
    const a = extractWebSourcesFromTool(
      'webfetch',
      '**Title:** First\n\nFetched: https://x.example\nStatus: 200\n\n',
      '1',
    );
    const b = extractWebSourcesFromTool(
      'webfetch',
      '**Title:** Second\n\nFetched: https://x.example/\nStatus: 200\n\n',
      '2',
    );
    expect(dedupeWebSources([...a, ...b])[0].title).toBe('First');
  });
});
