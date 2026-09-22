import { useCallback, useEffect, useState } from "react";
import { mailClient } from "./data/client";
import type { SplitInbox, SplitInboxMatchKind } from "./domain";
import { logBackgroundFailure } from "./errors";

/**
 * Owns the split inbox catalog. `loaded` flips once the first listing
 * settles (successfully or not) so callers can tell "no split inboxes" apart
 * from "not fetched yet" before restoring a stored tab.
 */
export function useSplitInboxes() {
  const [splitInboxes, setSplitInboxes] = useState<SplitInbox[]>([]);
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(() => {
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
    const updated = await mailClient.updateSplitInbox(id, name);
    setSplitInboxes((current) =>
      current.map((splitInbox) => splitInbox.id === id ? { ...splitInbox, ...updated } : splitInbox),
    );
  }, []);

  const remove = useCallback(async (id: string) => {
    await mailClient.deleteSplitInbox(id);
    setSplitInboxes((current) => current.filter((splitInbox) => splitInbox.id !== id));
  }, []);

  const reorder = useCallback(async (ids: string[]) => {
    await mailClient.reorderSplitInboxes(ids);
    await refresh();
  }, [refresh]);

  return { splitInboxes, loaded, refresh, create, rename, remove, reorder };
}
