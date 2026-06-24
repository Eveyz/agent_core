export function roughTokenCount(text: string): number {
  // PERF-2: Avoid [...text] array allocation. Iterate code points directly.
  let chars = 0;
  let ascii = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    // Skip surrogate pair low half (already counted via high half)
    if (code >= 0xD800 && code <= 0xDBFF) {
      chars++;
      i++; // skip low surrogate
    } else if (code >= 0xDC00 && code <= 0xDFFF) {
      continue; // already counted
    } else {
      chars++;
    }
    if (code < 128) ascii++;
  }
  return Math.floor(ascii / 4) + Math.floor((chars - ascii) / 2);
}
