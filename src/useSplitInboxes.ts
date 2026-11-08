import { useCallback, useEffect, useMemo, useState } from "react";
import { mailClient } from "./data/client";
import type { SplitInbox, SplitInboxMatchKind } from "./domain";
import { orderedMailViews, MAIL_VIEW_ORDER_KEY, SAVED_VIEW_PREFIX } from "./mailViews";
import { readSavedSearches, saveSearch, updateSavedSearch, deleteSavedSearch, type SavedSearch } from "./savedSearches";
import { errorMessage, logBackgroundFailure } from "./errors";

/**
 * Owns the split inbox catalog. `loaded` flips once the first listing
 * settles (successfully or not) so callers can tell "no split inboxes" apart
 * from "not fetched yet" before restoring a stored tab.
 */
export function useSplitInboxes() {
  const [splitInboxes, setSplitInboxes] = useState<SplitInbox[]>([]);
  const [searches, setSearches] = useState<SavedSearch[]>([]);
  const [viewOrder, setViewOrder] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(() => {
    try { setSearches(readSavedSearches()); setError(null); } catch (reason) { setError(errorMessage(reason)); }
    return mailClient.listSplitInboxes()
      .then((next) => setSplitInboxes(next))
      .catch(logBackgroundFailure("Split inbox listing"))
      .finally(() => setLoaded(true));
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const create = useCallback(async (name: string, matchKind: SplitInboxMatchKind, matchValue: string, accountId: string) => {
    const created = await mailClient.createSplitInbox(name, matchKind, matchValue, accountId);
    setSplitInboxes((current) => [...current, created]);
  }, []);

  const rename = useCallback(async (id: string, name: string) => {
    if (id.startsWith(SAVED_VIEW_PREFIX)) { setSearches(updateSavedSearch(id.slice(SAVED_VIEW_PREFIX.length), { name })); return; }
    const updated = await mailClient.updateSplitInbox(id, name);
    setSplitInboxes((current) =>
      current.map((splitInbox) => splitInbox.id === id ? { ...splitInbox, ...updated } : splitInbox),
    );
  }, []);

  const remove = useCallback(async (id: string) => {
    if (id.startsWith(SAVED_VIEW_PREFIX)) { setSearches(deleteSavedSearch(id.slice(SAVED_VIEW_PREFIX.length))); return; }
    await mailClient.deleteSplitInbox(id);
    setSplitInboxes((current) => current.filter((splitInbox) => splitInbox.id !== id));
  }, []);

  const reorder = useCallback(async (ids: string[]) => {
    await mailClient.reorderSplitInboxes(ids.filter((id) => !id.startsWith(SAVED_VIEW_PREFIX)));
    if (ids.some((id) => id.startsWith(SAVED_VIEW_PREFIX))) localStorage.setItem(MAIL_VIEW_ORDER_KEY, JSON.stringify(ids));
    setViewOrder(ids);
    await refresh();
  }, [refresh]);

  const views = useMemo(() => orderedMailViews(splitInboxes, searches, viewOrder), [splitInboxes, searches, viewOrder]);
  const createSearch = useCallback(async (search: Omit<SavedSearch, "id">) => { setSearches(saveSearch(search)); }, []);
  const updateSearch = useCallback(async (id: string, search: Omit<SavedSearch, "id">) => {
    setSearches(updateSavedSearch(id.startsWith(SAVED_VIEW_PREFIX) ? id.slice(SAVED_VIEW_PREFIX.length) : id, search));
  }, []);
  return { splitInboxes, views, loaded, error, refresh, create, createSearch, updateSearch, rename, remove, reorder };
}
