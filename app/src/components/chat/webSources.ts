import type { TurnBlock } from '../../features/chat/types';

export type WebSource = {
  url: string;
  title: string;
  snippet?: string;
  siteName?: string;
  publishedAt?: string;
  faviconUrl?: string;
  imageUrl?: string;
  toolName: string;
  callId: string;
};

export const FEATURED_SOURCE_LIMIT = 3;

export function hostnameFromUrl(url: string): string | undefined {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return undefined;
  }
}

export function faviconForUrl(url: string): string | undefined {
  const host = hostnameFromUrl(url);
  if (!host) return undefined;
  return `https://www.google.com/s2/favicons?domain=${encodeURIComponent(host)}&sz=128`;
}

function isHttpUrl(url: string): boolean {
  return /^https?:\/\//i.test(url.trim());
}

function normalizeUrl(url: string): string {
  return url.trim();
}

function makeSource(
  partial: Omit<WebSource, 'faviconUrl' | 'siteName'> & {
    siteName?: string;
    faviconUrl?: string;
  },
): WebSource | null {
  const url = normalizeUrl(partial.url);
  if (!isHttpUrl(url)) return null;
  const siteName = partial.siteName || hostnameFromUrl(url);
  return {
    ...partial,
    url,
    title: partial.title.trim() || siteName || url,
    siteName,
    faviconUrl: partial.faviconUrl || faviconForUrl(url),
  };
}

function excerptSnippet(excerpts: unknown): string | undefined {
  if (!Array.isArray(excerpts)) return undefined;
  const parts = excerpts
    .filter((e): e is string => typeof e === 'string' && e.trim().length > 0)
    .map((e) => e.trim());
  if (parts.length === 0) return undefined;
  const joined = parts.join(' ').replace(/\s+/g, ' ');
  return joined.length > 220 ? `${joined.slice(0, 217)}…` : joined;
}

/** Parallel MCP / generic `{ results: [{ url, title, ... }] }` shape. */
export function parseResultsJson(
  result: string,
  toolName: string,
  callId: string,
): WebSource[] {
  const trimmed = result.trim();
  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) return [];

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    return [];
  }

  const results = Array.isArray(parsed)
    ? parsed
    : parsed && typeof parsed === 'object' && Array.isArray((parsed as { results?: unknown }).results)
      ? (parsed as { results: unknown[] }).results
      : null;

  if (!results) return [];

  const out: WebSource[] = [];
  for (const item of results) {
    if (!item || typeof item !== 'object') continue;
    const row = item as Record<string, unknown>;
    const url = typeof row.url === 'string' ? row.url : '';
    const title =
      (typeof row.title === 'string' && row.title) ||
      (typeof row.name === 'string' && row.name) ||
      '';
    const snippet =
      excerptSnippet(row.excerpts) ||
      (typeof row.content === 'string' ? row.content : undefined) ||
      (typeof row.description === 'string' ? row.description : undefined) ||
      (typeof row.snippet === 'string' ? row.snippet : undefined);
    const publishedAt =
      (typeof row.publish_date === 'string' && row.publish_date) ||
      (typeof row.published_at === 'string' && row.published_at) ||
      undefined;
    const imageUrl =
      (typeof row.image === 'string' && row.image) ||
      (typeof row.image_url === 'string' && row.image_url) ||
      (typeof row.thumbnail === 'string' && row.thumbnail) ||
      (typeof row.og_image === 'string' && row.og_image) ||
      undefined;
    const source = makeSource({
      url,
      title,
      snippet: snippet?.replace(/\s+/g, ' ').trim().slice(0, 220) || undefined,
      publishedAt,
      imageUrl: imageUrl ? absolutizeUrl(url, imageUrl) : undefined,
      toolName,
      callId,
    });
    if (source) out.push(source);
  }
  return out;
}

/** Tavily tool markdown: `1. **Title**\nURL: …\nContent: …` */
export function parseTavilyMarkdown(
  result: string,
  toolName: string,
  callId: string,
): WebSource[] {
  const out: WebSource[] = [];
  const blockRe =
    /\d+\.\s+\*\*(.+?)\*\*\s*\nURL:\s*(\S+)\s*\nContent:\s*([\s\S]*?)(?=\n\d+\.\s+\*\*|\s*$)/g;
  let match: RegExpExecArray | null;
  while ((match = blockRe.exec(result)) !== null) {
    const source = makeSource({
      url: match[2],
      title: match[1],
      snippet: match[3].replace(/\s+/g, ' ').trim().slice(0, 220) || undefined,
      toolName,
      callId,
    });
    if (source) out.push(source);
  }
  return out;
}

/** Built-in webfetch header: `**Title:**` / `Fetched: url` / optional `**Image:**`. */
export function parseWebfetchResult(
  result: string,
  toolName: string,
  callId: string,
): WebSource[] {
  const fetchedMatch = result.match(/^Fetched:\s*(\S+)/m) || result.match(/^Fetched URL:\s*(\S+)/m);
  if (!fetchedMatch) return [];

  const titleMatch = result.match(/\*\*Title:\*\*\s*(.+)/);
  const descMatch = result.match(/\*\*Description:\*\*\s*(.+)/);
  const siteMatch = result.match(/\*\*Source:\*\*\s*(.+)/);
  const imageMatch = result.match(/\*\*Image:\*\*\s*(\S+)/);
  const pageUrl = fetchedMatch[1];
  const imageUrl = imageMatch?.[1] ? absolutizeUrl(pageUrl, imageMatch[1].trim()) : undefined;

  const source = makeSource({
    url: pageUrl,
    title: titleMatch?.[1]?.trim() || '',
    snippet: descMatch?.[1]?.trim(),
    siteName: siteMatch?.[1]?.trim(),
    imageUrl,
    toolName,
    callId,
  });
  return source ? [source] : [];
}

function absolutizeUrl(base: string, href: string): string {
  const trimmed = href.trim();
  if (/^https?:\/\//i.test(trimmed)) return trimmed;
  try {
    return new URL(trimmed, base).toString();
  } catch {
    return trimmed;
  }
}

function isNamedWebTool(name: string): boolean {
  return (
    name === 'tavily_search' ||
    name === 'webfetch' ||
    name.endsWith('__web_search') ||
    name.endsWith('__web_fetch')
  );
}

export function extractWebSourcesFromTool(
  name: string,
  result: string,
  callId: string,
): WebSource[] {
  if (!result || !name) return [];

  const fromJson = parseResultsJson(result, name, callId);
  if (fromJson.length > 0) return fromJson;

  if (name === 'tavily_search') {
    return parseTavilyMarkdown(result, name, callId);
  }
  if (name === 'webfetch' || name.endsWith('__web_fetch')) {
    return parseWebfetchResult(result, name, callId);
  }

  // Named search tools with non-JSON / non-tavily text: skip rather than URL-scrape.
  if (isNamedWebTool(name)) return [];
  return [];
}

/** Deduplicate by URL, preserving first-seen order. */
export function dedupeWebSources(sources: WebSource[]): WebSource[] {
  const seen = new Set<string>();
  const out: WebSource[] = [];
  for (const s of sources) {
    const key = s.url.replace(/\/$/, '').toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(s);
  }
  return out;
}

export function extractWebSourcesFromBlocks(blocks: TurnBlock[]): WebSource[] {
  const collected: WebSource[] = [];
  for (const block of blocks) {
    if (block.type !== 'tool' || block.is_error || !block.result) continue;
    // Prefer structured JSON from any tool; named web tools also get format parsers.
    const fromJson = parseResultsJson(block.result, block.name, block.call_id);
    if (fromJson.length > 0) {
      collected.push(...fromJson);
      continue;
    }
    if (!isNamedWebTool(block.name)) continue;
    collected.push(...extractWebSourcesFromTool(block.name, block.result, block.call_id));
  }
  return dedupeWebSources(collected);
}

export function featuredWebSources(
  sources: WebSource[],
  limit: number = FEATURED_SOURCE_LIMIT,
): WebSource[] {
  return sources.slice(0, Math.max(0, limit));
}

export function extractWebSourcesFromEntries(
  entries: Array<{ type?: string; blocks?: TurnBlock[] }>,
): WebSource[] {
  const collected: WebSource[] = [];
  for (const entry of entries) {
    if (entry.type !== 'turn' || !entry.blocks) continue;
    collected.push(...extractWebSourcesFromBlocks(entry.blocks));
  }
  return dedupeWebSources(collected);
}

export function formatRelativePublishedAt(publishedAt?: string): string | undefined {
  if (!publishedAt) return undefined;
  const parsed = Date.parse(publishedAt);
  if (Number.isNaN(parsed)) return publishedAt;
  const diffMs = Date.now() - parsed;
  const dayMs = 86_400_000;
  if (diffMs < dayMs && diffMs >= 0) return 'Today';
  const days = Math.floor(diffMs / dayMs);
  if (days === 1) return '1 day ago';
  if (days > 1 && days < 30) return `${days} days ago`;
  if (days >= 30 && days < 365) {
    const months = Math.floor(days / 30);
    return months === 1 ? '1 month ago' : `${months} months ago`;
  }
  try {
    return new Date(parsed).toLocaleDateString();
  } catch {
    return publishedAt;
  }
}
