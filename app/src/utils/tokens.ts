export function roughTokenCount(text: string): number {
  const chars = [...text].length;
  const ascii = [...text].filter((c) => c.charCodeAt(0) < 128).length;
  return Math.floor(ascii / 4) + Math.floor((chars - ascii) / 2);
}
