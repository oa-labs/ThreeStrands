/**
 * Recent mail searches, kept per device for the search box's suggestions.
 * Deliberately outside the `threestrands.settings.` namespace and the
 * settings-transfer allowlist: search history is local convenience, not a
 * preference to carry between devices. Storage can be unavailable (private
 * windows, blocked site data), so every access tolerates failure.
 */

export const RECENT_SEARCHES_KEY = "threestrands.recentSearches";
export const MAX_RECENT_SEARCHES = 8;

export function readRecentSearches(): string[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(RECENT_SEARCHES_KEY) ?? "[]");
    return Array.isArray(parsed)
      ? parsed.filter((entry): entry is string => typeof entry === "string" && Boolean(entry.trim())).slice(0, MAX_RECENT_SEARCHES)
      : [];
  } catch {
    return [];
  }
}

/** Moves `query` to the front, dropping a case-insensitive duplicate, and returns the new list. */
export function recordRecentSearch(query: string): string[] {
  const trimmed = query.trim();
  const current = readRecentSearches();
  if (!trimmed) return current;
  const next = [trimmed, ...current.filter((entry) => entry.toLocaleLowerCase() !== trimmed.toLocaleLowerCase())]
    .slice(0, MAX_RECENT_SEARCHES);
  try {
    localStorage.setItem(RECENT_SEARCHES_KEY, JSON.stringify(next));
  } catch {
    // The list still updates for this session's suggestions.
  }
  return next;
}
