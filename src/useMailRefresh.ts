import { useCallback, useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import type { RecoveryStatus, SyncStatus, UnreadCounts } from "./domain";
import type { SyncDiagnosticsActions } from "./settingsPanelTypes";
import { mailClient } from "./data/client";
import { useMailSyncActivity } from "./useMailSyncActivity";
import { createForegroundRefreshController } from "./foregroundRefresh";
import { logBackgroundFailure } from "./errors";
import type { useMailboxThreads } from "./useMailboxThreads";

export function useUnreadCounts(syncStatus: SyncStatus | null, setSyncStatus: Dispatch<SetStateAction<SyncStatus | null>>) {
  const [unreadCounts, setUnreadCounts] = useState<UnreadCounts>({});
  const mailSyncActive = useMailSyncActivity(() => {
    void mailClient.syncStatus().then(setSyncStatus).catch(logBackgroundFailure("Sync status refresh"));
  });
  const checkingMail = mailSyncActive || syncStatus?.state === "syncing";

  const refreshUnreadCounts = useCallback(() => {
    void mailClient.listUnreadCounts().then(setUnreadCounts).catch(logBackgroundFailure("Unread count refresh"));
  }, []);
  useEffect(refreshUnreadCounts, [refreshUnreadCounts]);
  return { unreadCounts, checkingMail, refreshUnreadCounts };
}

type Options = Pick<ReturnType<typeof useMailboxThreads>, "query" | "loadThreadsRef"> & {
  setSyncStatus: Dispatch<SetStateAction<SyncStatus | null>>;
  refreshTaskIndicators: () => Promise<void>;
};

export function useMailRefresh({ query, loadThreadsRef, setSyncStatus, refreshTaskIndicators }: Options) {
  const [recoveryStatus, setRecoveryStatus] = useState<RecoveryStatus | null>(null);
  useEffect(() => {
    // One-shot: this only ever reflects what happened during this app
    // launch's database open, so there's nothing to refresh later.
    void mailClient.recoveryStatus().then(setRecoveryStatus).catch(logBackgroundFailure("Recovery status check"));
  }, []);
  const syncDiagnostics = useMemo<SyncDiagnosticsActions>(() => ({
    retryFailed: async () => {
      setSyncStatus(await mailClient.retryFailedMutations());
      void mailClient.flushPending().then(setSyncStatus).catch(logBackgroundFailure("Pending mutation flush"));
    },
    dismissProblems: async () => setSyncStatus(await mailClient.dismissSyncProblems()),
    dismissRecovery: () => setRecoveryStatus(null),
  }), [setSyncStatus]);
  const refreshMail = useCallback(() => {
    setSyncStatus((current) => current ? { ...current, state: "syncing" } : current);
    void mailClient.sync()
      .then((status) => {
        setSyncStatus(status);
      })
      .catch(async () => {
        try {
          setSyncStatus(await mailClient.syncStatus());
        } catch {
          setSyncStatus((current) => current ? { ...current, state: "error" } : current);
        }
      })
      // A different account may still have completed when another failed.
      // Always repaint from the local cache after an all-account refresh.
      .finally(() => {
        void loadThreadsRef.current(query);
        void mailClient.reconcileTasks().catch(logBackgroundFailure("Task reconciliation"));
        void refreshTaskIndicators();
      });
  }, [loadThreadsRef, query, refreshTaskIndicators, setSyncStatus]);

  const refreshMailRef = useRef(refreshMail);
  refreshMailRef.current = refreshMail;

  useEffect(() => {
    let timer = 0;
    const flushIfInactive = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (document.visibilityState === "visible" && document.hasFocus()) return;
        void mailClient.flushPending().then(setSyncStatus).catch(logBackgroundFailure("Pending mutation flush"));
      }, 150);
    };
    const catchUp = createForegroundRefreshController(() => {
      refreshMailRef.current();
    });
    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        catchUp.onBackground();
        flushIfInactive();
      } else {
        catchUp.onForeground();
      }
    };
    const onBlur = () => {
      catchUp.onBackground();
      flushIfInactive();
    };
    const onFocus = () => catchUp.onForeground();
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("blur", onBlur);
    window.addEventListener("focus", onFocus);
    window.addEventListener("pageshow", onFocus);
    return () => {
      window.clearTimeout(timer);
      catchUp.dispose();
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("pageshow", onFocus);
    };
  }, [setSyncStatus]);

  return { refreshMail, recoveryStatus, syncDiagnostics };
}
