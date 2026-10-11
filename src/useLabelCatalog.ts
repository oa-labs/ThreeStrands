import { useCallback, useEffect, useRef, useState } from "react";
import type { Account, Label } from "./domain";
import { mailClient } from "./data/client";
import { useMailSyncActivity } from "./useMailSyncActivity";

export function useLabelCatalog(accounts: Account[], detailAccountId: string | undefined, labelTargetAccountId: string | undefined, reloadThreads: () => Promise<void>) {
  // Gmail user-label ids (for example `Label_18`) are only meaningful within
  // an account. Keep the catalogs separate so the same id in two accounts
  // cannot be displayed with the wrong account's label name.
  const [labelsByAccount, setLabelsByAccount] = useState<Record<string, Label[]>>({});
  const requests = useRef(new Map<string, object>());
  useEffect(() => {
    const pending = requests.current;
    return () => { pending.clear(); };
  }, []);

  const refreshLabels = useCallback(async (accountId: string) => {
    const request = {};
    requests.current.set(accountId, request);
    try {
      const accountLabels = await mailClient.listLabels(accountId);
      if (requests.current.get(accountId) !== request) return;
      setLabelsByAccount((catalogs) => ({ ...catalogs, [accountId]: accountLabels }));
    } catch {
      // Keep the last catalog while offline; the next sync or manager open
      // retries without turning a failed listing into a render/fetch loop.
    } finally {
      if (requests.current.get(accountId) === request) requests.current.delete(accountId);
    }
  }, []);

  // LIST can discover folders after startup, including on a sync with no
  // new messages. Refresh on every account's finish, not just unread changes.
  useMailSyncActivity((accountId) => { void refreshLabels(accountId); });
  useEffect(() => {
    if (labelTargetAccountId) void refreshLabels(labelTargetAccountId);
  }, [labelTargetAccountId, refreshLabels]);

  useEffect(() => {
    // Warm every connected account's label catalog, not just ones whose
    // threads happen to have been opened — otherwise a screen that lists
    // labels across accounts (e.g. the Split Inboxes label picker) can look
    // incomplete simply because that account hasn't been visited yet.
    const neededAccountIds = new Set(
      [detailAccountId, labelTargetAccountId, ...accounts.map((account) => account.email)].filter(
        (accountId): accountId is string => Boolean(accountId)
          && !labelsByAccount[accountId as string] && !requests.current.has(accountId as string),
      ),
    );
    for (const accountId of neededAccountIds) void refreshLabels(accountId);
  }, [detailAccountId, labelTargetAccountId, labelsByAccount, accounts, refreshLabels]);

  const createLabel = useCallback(async (name: string) => {
    if (!labelTargetAccountId) throw new Error("No account selected for this label");
    const label = await mailClient.createLabel(name, labelTargetAccountId);
    requests.current.delete(labelTargetAccountId);
    setLabelsByAccount((current) => ({ ...current, [labelTargetAccountId]: [...(current[labelTargetAccountId] ?? []), label] }));
    return label;
  }, [labelTargetAccountId]);
  const deleteLabel = useCallback(async (id: string) => {
    if (!labelTargetAccountId) return;
    await mailClient.deleteLabel(id, labelTargetAccountId);
    requests.current.delete(labelTargetAccountId);
    setLabelsByAccount((current) => ({ ...current, [labelTargetAccountId]: (current[labelTargetAccountId] ?? []).filter((label) => label.id !== id) }));
    await reloadThreads();
  }, [labelTargetAccountId, reloadThreads]);
  const renameLabel = useCallback(async (id: string, name: string) => {
    if (!labelTargetAccountId) return;
    const updated = await mailClient.updateLabel(id, name, labelTargetAccountId);
    requests.current.delete(labelTargetAccountId);
    setLabelsByAccount((current) => ({ ...current, [labelTargetAccountId]: (current[labelTargetAccountId] ?? []).map((label) => label.id === id ? { ...label, ...updated } : label) }));
  }, [labelTargetAccountId]);
  return { labelsByAccount, createLabel, deleteLabel, renameLabel };
}
