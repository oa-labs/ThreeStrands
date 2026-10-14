import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { removeSyncedMailAccount } from "./replicatedSync";
import { mailClient } from "./data/client";
import type {
  Account,
  AuthStatus,
  MailboxUnreadCounts,
  SyncStatus,
} from "./domain";
import { readSeenAvailableAccounts, readSelectedAccountId, saveSeenAvailableAccounts, saveSelectedAccountId } from "./settings";
import { logBackgroundFailure } from "./errors";

export const ACCOUNT_STATUS_REFRESH_MS = 15_000;

/**
 * Coordinates account identity, authentication, sync status, and account-
 * scoped unread counts. Mailbox content remains outside this hook so changing
 * account metadata cannot implicitly navigate or replace the current list.
 */
export function useAccounts(settingsOpen: boolean) {
  // Every catalogued account, including ones only set up on other devices.
  const [catalog, setCatalog] = useState<Account[]>([]);
  const accounts = useMemo(() => catalog.filter((account) => account.onThisDevice), [catalog]);
  const availableAccounts = useMemo(() => catalog.filter((account) => !account.onThisDevice), [catalog]);
  const [seenAvailable, setSeenAvailable] = useState<string[]>(readSeenAvailableAccounts);
  const newlyAvailableAccounts = useMemo(
    () => availableAccounts.filter((account) => !seenAvailable.includes(account.email)),
    [availableAccounts, seenAvailable],
  );
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
        setCatalog(next);
        setActiveAccountId((current) =>
          current === null || next.some((account) => account.email === current && account.onThisDevice)
            ? current
            : null,
        );
      })
      // A transient status request failure should not hide account controls.
      .catch(logBackgroundFailure("Account status refresh"));
  }, []);

  const refreshMailboxUnreadCounts = useCallback((accountOverride?: string | null) => {
    const accountId = (accountOverride !== undefined ? accountOverride : activeAccountId) ?? undefined;
    void mailClient.mailboxUnreadCounts(accountId).then(setMailboxUnreadCounts).catch(logBackgroundFailure("Mailbox unread count refresh"));
  }, [activeAccountId]);

  /** Reorders the accounts set up here; accounts only available from other
   * devices keep their place after them. */
  const reorderAccounts = useCallback(async (emails: string[]) => {
    const accountsByEmail = new Map(accounts.map((account) => [account.email, account]));
    const reordered = emails
      .map((email) => accountsByEmail.get(email))
      .filter((account): account is Account => account !== undefined);
    if (reordered.length !== accounts.length) return;

    const previous = catalog;
    setCatalog([...reordered, ...availableAccounts]);
    try {
      await mailClient.reorderAccounts([...emails, ...availableAccounts.map((account) => account.email)]);
      await refreshAccounts();
    } catch (error) {
      setCatalog(previous);
      throw error;
    }
  }, [accounts, availableAccounts, catalog, refreshAccounts]);

  /** Records that the user has seen every account currently available from
   * other devices, so the arrival notice does not repeat. */
  const acknowledgeAvailableAccounts = useCallback(() => {
    setSeenAvailable((current) => {
      const next = [...new Set([...current, ...availableAccounts.map((account) => account.email)])];
      saveSeenAvailableAccounts(next);
      return next;
    });
  }, [availableAccounts]);

  const addAccount = useCallback(async () => {
    // Gmail is the only provider the account UI offers so far.
    await mailClient.addAccount("gmail");
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

  const setAccountNickname = useCallback(async (email: string, nickname: string | null) => {
    await mailClient.setAccountNickname(email, nickname);
    await refreshAccounts();
  }, [refreshAccounts]);

  useEffect(() => {
    saveSelectedAccountId(activeAccountId);
  }, [activeAccountId]);

  useEffect(refreshMailboxUnreadCounts, [refreshMailboxUnreadCounts]);

  useEffect(() => {
    const interval = window.setInterval(refreshAccounts, ACCOUNT_STATUS_REFRESH_MS);
    return () => window.clearInterval(interval);
  }, [refreshAccounts]);

  useEffect(() => {
    if (settingsOpen) void refreshAccounts();
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
    availableAccounts,
    newlyAvailableAccounts,
    acknowledgeAvailableAccounts,
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
    setAccountNickname,
    setAccountColor,
    reorderAccounts,
  };
}
