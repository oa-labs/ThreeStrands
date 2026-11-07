import { MESSAGE_FILTER_OPTIONS, type MessageFilterKind } from "./messageFilters";

/** Named mail searches are device-local, outside settings transfer and sync. */
export const SAVED_SEARCHES_KEY = "threestrands.savedSearches";

export type SavedSearch = {
  id: string;
  name: string;
  query: string;
  /** null deliberately means all connected accounts, including future additions. */
  accountId: string | null;
  inboxOnly: boolean;
  messageFilters: MessageFilterKind[];
};

export function readSavedSearches(): SavedSearch[] {
  const raw = localStorage.getItem(SAVED_SEARCHES_KEY);
  if (!raw) return [];
  const data: unknown = JSON.parse(raw);
  if (!data || typeof data !== "object" || !("version" in data) || data.version !== 1
    || !("searches" in data) || !Array.isArray(data.searches)) {
    throw new Error("Saved searches could not be read. Your saved data has been kept.");
  }
  const ids = new Set<string>();
  return data.searches.map((entry: unknown) => {
    if (!entry || typeof entry !== "object" || !("id" in entry) || typeof entry.id !== "string" || !entry.id
      || ids.has(entry.id) || !("name" in entry) || typeof entry.name !== "string" || !entry.name.trim()
      || !("query" in entry) || typeof entry.query !== "string" || !entry.query.trim()
      || !("accountId" in entry) || !(entry.accountId === null || (typeof entry.accountId === "string" && entry.accountId.trim()))
      || !("inboxOnly" in entry) || typeof entry.inboxOnly !== "boolean") {
      throw new Error("Saved searches could not be read. Your saved data has been kept.");
    }
    const messageFilters = "messageFilters" in entry ? entry.messageFilters : [];
    if (!Array.isArray(messageFilters) || !messageFilters.every((value): value is MessageFilterKind =>
      MESSAGE_FILTER_OPTIONS.some((option) => option.kind === value))) {
      throw new Error("Saved searches could not be read. Your saved data has been kept.");
    }
    ids.add(entry.id);
    return { id: entry.id, name: entry.name, query: entry.query, accountId: entry.accountId, inboxOnly: entry.inboxOnly, messageFilters };
  });
}

export function saveSearch(search: Omit<SavedSearch, "id" | "messageFilters"> & { messageFilters?: MessageFilterKind[] }): SavedSearch[] {
  const current = readSavedSearches();
  const name = search.name.trim();
  const query = search.query.trim();
  if (!name || !query) throw new Error("Enter a name and a mail search.");
  if (search.accountId !== null && !search.accountId.trim()) throw new Error("Choose an account scope.");
  if (current.some((entry) => entry.accountId === search.accountId && entry.name.toLocaleLowerCase() === name.toLocaleLowerCase())) {
    throw new Error("A saved search with this name already exists for this account scope.");
  }
  const next = [...current, { ...search, messageFilters: search.messageFilters ?? [], name, query, id: crypto.randomUUID() }];
  localStorage.setItem(SAVED_SEARCHES_KEY, JSON.stringify({ version: 1, searches: next }));
  return next;
}

export function deleteSavedSearch(id: string): SavedSearch[] {
  const next = readSavedSearches().filter((entry) => entry.id !== id);
  localStorage.setItem(SAVED_SEARCHES_KEY, JSON.stringify({ version: 1, searches: next }));
  return next;
}
