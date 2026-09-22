import { useCallback, useEffect, useRef, useState } from "react";
import { removeSyncedMailAccount } from "./replicatedSync";
import { mailClient } from "./data/client";
import type {
  Account,
  AuthStatus,
  MailboxUnreadCounts,
  SyncStatus,
} from "./domain";
import { readSelectedAccountId, saveSelectedAccountId } from "./settings";
import { logBackgroundFailure } from "./errors";

export const ACCOUNT_STATUS_REFRESH_MS = 15_000;

/**
 * Coordinates account identity, authentication, sync status, and account-
 * scoped unread counts. Mailbox content remains outside this hook so changing
 * account metadata cannot implicitly navigate or replace the current list.
 */
export function useAccounts(settingsOpen: boolean) {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [authStatus, setAuthStatus] = useState<AuthStatus | null>(null);
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null);
  const [activeAccountId, setActiveAccountId] = useState<string | null>(readSelectedAccountId);
  const [mailboxUnreadCounts, setMailboxUnreadCounts] = useState<MailboxUnreadCounts>({
    inbox: 0,
    splits: {},
  });
  const accountsRequest = useRef(0);

  const refreshAccounts = useCallback(() => {
    // Ignore stale responses when account edits trigger overlapping refreshes.
    const requestId = ++accountsRequest.current;
    return mailClient
      .listAccounts()
      .then((next) => {
        if (requestId !== accountsRequest.current) return;
        setAccounts(next);
        setActiveAccountId((current) =>
          current === null || next.some((account) => account.email === current)
            ? current
            : null,
        );
      })
      .catch(() => {
        if (requestId === accountsRequest.current) setAccounts([]);
      });
  }, []);

  const refreshMailboxUnreadCounts = useCallback((accountOverride?: string | null) => {
    const accountId = (accountOverride !== undefined ? accountOverride : activeAccountId) ?? undefined;
    void mailClient.mailboxUnreadCounts(accountId).then(setMailboxUnreadCounts).catch(logBackgroundFailure("Mailbox unread count refresh"));
  }, [activeAccountId]);

  const reorderAccounts = useCallback(async (emails: string[]) => {
    const accountsByEmail = new Map(accounts.map((account) => [account.email, account]));
    const reordered = emails
      .map((email) => accountsByEmail.get(email))
      .filter((account): account is Account => account !== undefined);
    if (reordered.length !== accounts.length) return;

    setAccounts(reordered);
    try {
      await mailClient.reorderAccounts(emails);
      await refreshAccounts();
    } catch (error) {
      setAccounts(accounts);
      throw error;
    }
  }, [accounts, refreshAccounts]);

  const addAccount = useCallback(async () => {
    await mailClient.addAccount();
    setAuthStatus(await mailClient.googleAuthStatus());
    await refreshAccounts();
  }, [refreshAccounts]);

  /** Resolves with whether the removed account was the active one. */
  const removeAccount = useCallback(async (email: string) => {
    await mailClient.removeAccount(email);
    const wasActive = activeAccountId === email;
    if (wasActive) setActiveAccountId(null);
    await refreshAccounts();
    setAuthStatus(await mailClient.googleAuthStatus());
    return { wasActive };
  }, [activeAccountId, refreshAccounts]);

  const removeAccountEverywhere = useCallback(async (email: string) => {
    await removeSyncedMailAccount(email);
    await refreshAccounts();
  }, [refreshAccounts]);

  // Account, auth, and sync state refresh even when reconnecting fails, so a
  // cancelled or rejected sign-in still reflects the credential's real state.
  const reconnectAccount = useCallback(async (email: string) => {
    try {
      await mailClient.reconnectAccount(email);
    } finally {
      await refreshAccounts();
      setAuthStatus(await mailClient.googleAuthStatus());
      setSyncStatus(await mailClient.syncStatus());
    }
  }, [refreshAccounts]);

  const setAccountDisplayName = useCallback(async (email: string, displayName: string | null) => {
    await mailClient.setAccountDisplayName(email, displayName);
    await refreshAccounts();
  }, [refreshAccounts]);

  const setAccountColor = useCallback(async (email: string, color: string) => {
    await mailClient.setAccountColor(email, color);
    await refreshAccounts();
  }, [refreshAccounts]);

  useEffect(() => {
    saveSelectedAccountId(activeAccountId);
  }, [activeAccountId]);

  useEffect(refreshMailboxUnreadCounts, [refreshMailboxUnreadCounts]);

  useEffect(() => {
    if (!settingsOpen) return;
    void refreshAccounts();
    const interval = window.setInterval(refreshAccounts, ACCOUNT_STATUS_REFRESH_MS);
    return () => window.clearInterval(interval);
  }, [settingsOpen, refreshAccounts]);

  useEffect(() => {
    Promise.all([mailClient.syncStatus(), mailClient.googleAuthStatus()])
      .then(([status, auth]) => {
        setSyncStatus(status);
        setAuthStatus(auth);
      })
      .catch(() => {
        setSyncStatus((current) => current ? { ...current, state: "error" } : current);
      });
    void refreshAccounts();
  }, [refreshAccounts]);

  return {
    accounts,
    authStatus,
    setAuthStatus,
    syncStatus,
    setSyncStatus,
    activeAccountId,
    setActiveAccountId,
    mailboxUnreadCounts,
    refreshMailboxUnreadCounts,
    refreshAccounts,
    addAccount,
    removeAccount,
    removeAccountEverywhere,
    reconnectAccount,
    setAccountDisplayName,
    setAccountColor,
    reorderAccounts,
  };
}
