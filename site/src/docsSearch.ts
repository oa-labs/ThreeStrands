export type SearchEntry = { title: string; section: string; href: string; text: string };
export type SearchMatch = SearchEntry & { score: number; snippet: string };

const SYNONYMS: Record<string, string[]> = {
  backup: ["export"], backups: ["export"], delayed: ["schedule"], scheduled: ["schedule"],
  attachments: ["attachment"], attachment: ["files"], syncing: ["sync"],
  migrate: ["replace"], migration: ["replace"], offline: ["cache"],
};

export function searchTerms(query: string): string[][] {
  return (query.toLowerCase().match(/[\p{L}\p{N}]+/gu) ?? []).slice(0, 12)
    .map((term) => [term, ...(SYNONYMS[term] ?? [])]);
}

/** Require every query word, while allowing known synonyms for each word. */
export function searchDocs(entries: SearchEntry[], query: string): SearchMatch[] {
  const terms = searchTerms(query);
  if (!terms.length) return [];
  return entries.flatMap((entry) => {
    const title = entry.title.toLowerCase();
    const section = entry.section.toLowerCase();
    const body = entry.text.toLowerCase();
    let score = 0;
    for (const alternatives of terms) {
      const best = Math.max(...alternatives.map((term) =>
        (title.includes(term) ? 5 : 0) + (section.includes(term) ? 8 : 0) + (body.includes(term) ? 1 : 0)));
      if (!best) return [];
      score += best;
    }
    const position = Math.max(0, Math.min(...terms.flat().map((term) => body.indexOf(term)).filter((index) => index >= 0)) - 60);
    const start = Number.isFinite(position) ? position : 0;
    const snippet = `${start ? "…" : ""}${entry.text.slice(start, start + 220)}${entry.text.length > start + 220 ? "…" : ""}`;
    return [{ ...entry, score, snippet }];
  }).sort((a, b) => b.score - a.score || a.href.localeCompare(b.href)).slice(0, 12);
}
