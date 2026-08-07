# Streamdown prototype notes

Status: evaluated, not wired into the production chat renderer.

Run with `npm run prototype:streamdown`, then compare:

- `?variant=A` — current `MarkdownContent`
- `?variant=B` — Streamdown with code and math

Useful query flags: `autoplay=1`, `speed=4`, `full=1`, and `cursor=<number>`.

## Production trial flag

The current renderer remains the default. Enable Streamdown for assistant
messages with `VITE_STREAMDOWN_ASSISTANT=true`, or override one installation
from DevTools and reload:

```js
localStorage.setItem('agent_core_streamdown_assistant', 'true');  // enable
localStorage.setItem('agent_core_streamdown_assistant', 'false'); // force fallback
localStorage.removeItem('agent_core_streamdown_assistant');   // use build default
```

## Browser sample (769 characters, 86 commits, 4x stream speed)

| Variant | commit p95 | commit max | resize callbacks | DOM mutation batches |
| --- | ---: | ---: | ---: | ---: |
| A | 3.30 ms | 5.90 ms | 17 | 94 |
| B | 2.60 ms | 9.10 ms | 5 | 271 |

These are directional single-browser measurements, not a release benchmark.

## Findings

- Streamdown handles unfinished input, math, tables and unsafe images well. The unsafe image becomes a blocked-image label.
- The app does not use Tailwind, while Streamdown's generated elements and controls rely heavily on Tailwind utility classes. A plain-CSS adapter is required; the full control set is not production-ready with the current adapter.
- Streamdown renders bold text as a styled `span` instead of a semantic `strong`, so the adapter must restore the visual weight and accessibility semantics should be reviewed.
- The matched variant causes fewer resize callbacks but substantially more DOM mutation batches than the current renderer.
- The production trial reuses the app's Shiki 4 highlighter instead of installing `@streamdown/code`, avoiding duplicate language chunks.

## Recommendation

Do not replace `MarkdownContent` wholesale yet. Trial variant B behind a feature flag on assistant messages only. Mermaid, CJK and the full controls are intentionally excluded.
