import { useCallback, useEffect, useState } from "react";
import type { Account, Label } from "./domain";
import { mailClient } from "./data/client";

export function useLabelCatalog(accounts: Account[], detailAccountId: string | undefined, labelTargetAccountId: string | undefined, reloadThreads: () => Promise<void>) {
  // Gmail user-label ids (for example `Label_18`) are only meaningful within
  // an account. Keep the catalogs separate so the same id in two accounts
  // cannot be displayed with the wrong account's label name.
  const [labelsByAccount, setLabelsByAccount] = useState<Record<string, Label[]>>({});
  useEffect(() => {
    // Warm every connected account's label catalog, not just ones whose
    // threads happen to have been opened — otherwise a screen that lists
    // labels across accounts (e.g. the Split Inboxes label picker) can look
    // incomplete simply because that account hasn't been visited yet.
    const neededAccountIds = new Set(
      [detailAccountId, labelTargetAccountId, ...accounts.map((account) => account.email)].filter(
        (accountId): accountId is string => Boolean(accountId) && !labelsByAccount[accountId as string],
      ),
    );
    if (neededAccountIds.size === 0) return;
    let current = true;
    void Promise.all(
      [...neededAccountIds].map((accountId) =>
        mailClient
          .listLabels(accountId)
          .then((accountLabels) => [accountId, accountLabels] as const)
          .catch(() => null),
      ),
    ).then((results) => {
      if (!current) return;
      setLabelsByAccount((catalogs) => {
        const next = { ...catalogs };
        for (const result of results) {
          if (result) next[result[0]] = result[1];
        }
        return next;
      });
    });
    return () => {
      current = false;
    };
  }, [detailAccountId, labelTargetAccountId, labelsByAccount, accounts]);

  const createLabel = useCallback(async (name: string) => {
    if (!labelTargetAccountId) throw new Error("No account selected for this label");
    const label = await mailClient.createLabel(name, labelTargetAccountId);
    setLabelsByAccount((current) => ({ ...current, [labelTargetAccountId]: [...(current[labelTargetAccountId] ?? []), label] }));
    return label;
  }, [labelTargetAccountId]);
  const deleteLabel = useCallback(async (id: string) => {
    if (!labelTargetAccountId) return;
    await mailClient.deleteLabel(id, labelTargetAccountId);
    setLabelsByAccount((current) => ({ ...current, [labelTargetAccountId]: (current[labelTargetAccountId] ?? []).filter((label) => label.id !== id) }));
    await reloadThreads();
  }, [labelTargetAccountId, reloadThreads]);
  const renameLabel = useCallback(async (id: string, name: string) => {
    if (!labelTargetAccountId) return;
    const updated = await mailClient.updateLabel(id, name, labelTargetAccountId);
    setLabelsByAccount((current) => ({ ...current, [labelTargetAccountId]: (current[labelTargetAccountId] ?? []).map((label) => label.id === id ? { ...label, ...updated } : label) }));
  }, [labelTargetAccountId]);
  return { labelsByAccount, createLabel, deleteLabel, renameLabel };
}
