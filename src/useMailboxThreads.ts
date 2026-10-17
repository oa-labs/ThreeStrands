import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { listen } from "@tauri-apps/api/event";
import type { MailboxKind } from "./commands";
import type { SearchReport, Thread } from "./domain";
import { mailClient } from "./data/client";
import { searchStatusMessage } from "./searchStatusCopy";
import { errorMessage, logBackgroundFailure } from "./errors";

const SEARCH_PAGE_SIZE = 50;
// While a Gmail backfill scan is running, matches land in the local cache
// incrementally (see sync.rs's flushed ingest_threads batches), so poll
// local search at this cadence rather than waiting for the whole scan to
// finish before a match becomes visible.
const REMOTE_SEARCH_POLL_MS = 1200;
// Local search already runs on SearchField's short typing debounce. A server
// search costs a provider round trip per query, so it waits for a longer
// pause after the local results land instead of firing on every word.
export const REMOTE_SEARCH_DELAY_MS = 600;

type Options = {
  activeAccountId: string | null;
  mailbox: MailboxKind;
  activeSplitInboxId: string | null;
  setSelectedId: Dispatch<SetStateAction<string | null>>;
  refreshUnreadCounts: () => void;
  refreshMailboxUnreadCounts: () => void;
};

/** Owns mailbox pages and local/remote search, including stale-response guards. */
export function useMailboxThreads({
  activeAccountId, mailbox, activeSplitInboxId, setSelectedId, refreshUnreadCounts,
  refreshMailboxUnreadCounts,
}: Options) {
  const [threads, setThreads] = useState<Thread[]>([]);
  const [query, setQuery] = useState("");
  const queryRef = useRef(query);
  queryRef.current = query;
  const [searchOpen, setSearchOpen] = useState(false);
  // Search covers all mail (minus Trash and Spam) unless the user narrows it
  // to the inbox. `includeArchived` is derived so an empty search box still
  // reads as the plain inbox to triage and mutation code.
  const [inboxOnly, setInboxOnly] = useState(false);
  const includeArchived = Boolean(query.trim()) && !inboxOnly;
  const [remoteSearchState, setRemoteSearchState] = useState<"idle" | "searching" | "error">("idle");
  // Phase 5 run 5: the ONE quiet status line for the current search (a partial,
  // skipped, or no-matches outcome worded in `searchStatusCopy`), or null when
  // there is nothing to say. Cleared whenever the query, account, or "include
  // archived" changes — never carried from an earlier search.
  const [searchStatus, setSearchStatus] = useState<string | null>(null);
  const [hasMoreResults, setHasMoreResults] = useState(false);
  const [loading, setLoading] = useState(true);
  const [mailboxError, setMailboxError] = useState("");
  const threadsRequest = useRef(0);
  const contextOpenedThreadRef = useRef<string | null>(null);
  const loadingMore = useRef(false);
  const [loadingMoreState, setLoadingMoreState] = useState(false);
  const remoteSearchKeyRef = useRef<string | null>(null);
  const remoteSearchRequestRef = useRef(0);
  const remoteSearchPromiseRef = useRef<Promise<SearchReport> | null>(null);
  const remoteSearchInFlightRef = useRef(false);
  const remoteSearchTimerRef = useRef<number | null>(null);
  const cancelRemoteSearch = useCallback(() => {
    if (remoteSearchTimerRef.current !== null) window.clearTimeout(remoteSearchTimerRef.current);
    remoteSearchTimerRef.current = null;
    remoteSearchKeyRef.current = null;
    remoteSearchRequestRef.current += 1;
    remoteSearchPromiseRef.current = null;
    remoteSearchInFlightRef.current = false;
    setRemoteSearchState("idle");
    setSearchStatus(null);
  }, []);
  const fetchMailboxPage = useCallback((box: MailboxKind, accountId: string | undefined, offset: number) => {
    if (box === "allMail") return mailClient.listAllMailPage(accountId, offset, SEARCH_PAGE_SIZE);
    if (box === "sent") return mailClient.listSentPage(accountId, offset, SEARCH_PAGE_SIZE);
    if (box === "trash") return mailClient.listTrashPage(accountId, offset, SEARCH_PAGE_SIZE);
    if (box === "split" && activeSplitInboxId) {
      return mailClient.listSplitInboxPage(activeSplitInboxId, offset, SEARCH_PAGE_SIZE);
    }
    return mailClient.listThreadsPage(accountId, offset, SEARCH_PAGE_SIZE);
  }, [activeSplitInboxId]);

  const loadThreads = useCallback(async (search: string, accountOverride?: string | null, mailboxOverride?: MailboxKind) => {
    const requestId = ++threadsRequest.current;
    const box = mailboxOverride ?? mailbox;
    if (box === "drafts" || box === "outbox" || (box === "split" && !activeSplitInboxId)) {
      setThreads([]);
      setSelectedId(null);
      setHasMoreResults(false);
      setMailboxError("");
      return;
    }
    setMailboxError("");
    const trimmed = search.trim();
    const accountId = (accountOverride !== undefined ? accountOverride : activeAccountId) ?? undefined;
    const searchableMailbox = box === "inbox" || box === "split";
    const searchAllMail = !inboxOnly;
    if (!searchableMailbox || !trimmed || !searchAllMail) cancelRemoteSearch();
    const commitPage = (page: { threads: Thread[]; hasMore: boolean }) => {
      if (requestId !== threadsRequest.current) return false;
      setMailboxError("");
      setThreads(page.threads);
      setHasMoreResults(page.hasMore);
      refreshUnreadCounts();
      refreshMailboxUnreadCounts();
      setSelectedId((current) =>
        current && (page.threads.some((thread) => thread.id === current) || contextOpenedThreadRef.current === current)
          ? current
          : (page.threads[0]?.id ?? null),
      );
      return true;
    };
    try {
      if ((box === "inbox" || box === "split") && trimmed) {
        const searchRequest = {
          query: trimmed,
          limit: SEARCH_PAGE_SIZE,
          includeArchived: searchAllMail,
        };
        const localThreads = await mailClient.searchThreads(searchRequest, accountId);
        if (!commitPage({ threads: localThreads, hasMore: localThreads.length === SEARCH_PAGE_SIZE })) return;

        const remoteSearchKey = `${accountId ?? "all"}\u0000${trimmed}`;
        if (searchAllMail && remoteSearchKeyRef.current !== remoteSearchKey) {
          if (remoteSearchTimerRef.current !== null) window.clearTimeout(remoteSearchTimerRef.current);
          remoteSearchKeyRef.current = remoteSearchKey;
          const remoteRequestId = ++remoteSearchRequestRef.current;
          remoteSearchInFlightRef.current = true;
          setRemoteSearchState("searching");
          let pollInterval: number | undefined;
          const pollForRemoteMatches = () => {
            if (remoteRequestId !== remoteSearchRequestRef.current) return;
            void mailClient.searchThreads(searchRequest, accountId).then((polled) => {
              if (remoteRequestId !== remoteSearchRequestRef.current || requestId !== threadsRequest.current) return;
              commitPage({ threads: polled, hasMore: polled.length === SEARCH_PAGE_SIZE });
            });
          };
          // The promise is published now so a reload during the pause (a sync
          // landing, say) attaches to it below; the provider request itself
          // starts only once the query has held still. A superseded query's
          // promise simply never settles.
          const backfillPromise = new Promise<SearchReport>((resolve, reject) => {
            remoteSearchTimerRef.current = window.setTimeout(() => {
              remoteSearchTimerRef.current = null;
              if (remoteRequestId !== remoteSearchRequestRef.current) return;
              pollInterval = window.setInterval(pollForRemoteMatches, REMOTE_SEARCH_POLL_MS);
              mailClient.backfillSearchThreads(trimmed, accountId).then(resolve, reject);
            }, REMOTE_SEARCH_DELAY_MS);
          });
          remoteSearchPromiseRef.current = backfillPromise;
          void backfillPromise
            .then(async (report) => {
              window.clearInterval(pollInterval);
              if (remoteRequestId !== remoteSearchRequestRef.current) return;
              const refreshed = await mailClient.searchThreads(searchRequest, accountId);
              // The remoteRequestId guard also covers the REPORT: a stale
              // result from an earlier query (its id already superseded) never
              // sets the status line for the current one.
              if (remoteRequestId !== remoteSearchRequestRef.current) return;
              remoteSearchInFlightRef.current = false;
              if (requestId === threadsRequest.current) {
                commitPage({ threads: refreshed, hasMore: refreshed.length === SEARCH_PAGE_SIZE });
                setRemoteSearchState("idle");
                setSearchStatus(searchStatusMessage(report, refreshed.length === 0));
              }
            })
            .catch(() => {
              window.clearInterval(pollInterval);
              // Gmail-backed search is an enhancement to the local result,
              // not a reason to make search fail while offline.
              if (remoteSearchKeyRef.current === remoteSearchKey && remoteRequestId === remoteSearchRequestRef.current) {
                remoteSearchInFlightRef.current = false;
                remoteSearchKeyRef.current = null;
                setRemoteSearchState("error");
                setSearchStatus(null);
              }
            });
        } else if (searchAllMail && remoteSearchKeyRef.current === remoteSearchKey && remoteSearchInFlightRef.current) {
          const remoteRequestId = remoteSearchRequestRef.current;
          const remotePromise = remoteSearchPromiseRef.current;
          if (remotePromise) {
            void remotePromise.then(async (report) => {
              if (requestId !== threadsRequest.current || remoteRequestId !== remoteSearchRequestRef.current) return;
              const refreshed = await mailClient.searchThreads(searchRequest, accountId);
              if (requestId !== threadsRequest.current || remoteRequestId !== remoteSearchRequestRef.current) return;
              commitPage({ threads: refreshed, hasMore: refreshed.length === SEARCH_PAGE_SIZE });
              remoteSearchInFlightRef.current = false;
              setRemoteSearchState("idle");
              setSearchStatus(searchStatusMessage(report, refreshed.length === 0));
            }).catch(logBackgroundFailure("Remote search refresh"));
          }
        }
        return;
      }

      const page = await fetchMailboxPage(box, accountId, 0);
      commitPage(page);
    } catch (error) {
      if (requestId !== threadsRequest.current) return;
      setMailboxError(errorMessage(error));
    }
  }, [
    mailbox, activeSplitInboxId, activeAccountId, inboxOnly, setSelectedId,
    refreshUnreadCounts, refreshMailboxUnreadCounts, fetchMailboxPage, cancelRemoteSearch,
  ]);

  // Sync round trips can outlive a mailbox/split-inbox switch. Reading
  // loadThreads through a ref at resolution time (rather than closing over
  // whichever instance existed when the sync started) keeps a slow sync from
  // repainting the thread list for a view the user has since navigated away
  // from, even though the header already reflects the new view.
  const loadThreadsRef = useRef(loadThreads);
  loadThreadsRef.current = loadThreads;
  // Background accounts keep polling Gmail while a different account is
  // active in the UI; without this, their sidebar badge only catches up to
  // what the backend already knows the next time the mailbox reloads (e.g.
  // switching into that account). The active account's Inbox badge and
  // visible thread list need the same treatment, or a long-idle-but-focused
  // session can silently accumulate unread mail with no on-screen change
  // until some unrelated action (e.g. sending mail) happens to reload the
  // mailbox.
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const unlisten = listen<string>("unread-counts-changed", (event) => {
      refreshUnreadCounts();
      refreshMailboxUnreadCounts();
      // A null activeAccountId is the merged "All accounts" view, which
      // shows every account's mail, so any account's sync should refresh it.
      if (activeAccountId === null || event.payload === activeAccountId) {
        void loadThreadsRef.current(queryRef.current);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refreshUnreadCounts, refreshMailboxUnreadCounts, activeAccountId]);

  const loadMoreResults = useCallback(async () => {
    const trimmed = query.trim();
    if ((mailbox === "drafts" || mailbox === "outbox") || loadingMore.current) return;
    if (mailbox === "split" && !activeSplitInboxId) return;
    loadingMore.current = true;
    setLoadingMoreState(true);
    const requestId = threadsRequest.current;
    try {
      const accountId = activeAccountId ?? undefined;
      const page = trimmed
        ? await mailClient.searchThreads({
            query: trimmed,
            limit: SEARCH_PAGE_SIZE,
            offset: threads.length,
            includeArchived,
          }, accountId).then((items) => ({ threads: items, hasMore: items.length === SEARCH_PAGE_SIZE }))
        : await fetchMailboxPage(mailbox, accountId, threads.length);
      if (requestId !== threadsRequest.current) return;
      setThreads((current) => [...current, ...page.threads]);
      setHasMoreResults(page.hasMore);
    } catch (error) {
      if (requestId === threadsRequest.current) {
        setMailboxError(errorMessage(error));
      }
    } finally {
      loadingMore.current = false;
      setLoadingMoreState(false);
    }
  }, [query, threads.length, includeArchived, activeAccountId, mailbox, activeSplitInboxId, fetchMailboxPage]);

  useEffect(() => {
    // Invalidate an in-flight request as soon as the view inputs change. The
    // debounce below is intentionally only for starting the replacement
    // request; it must not leave an older search eligible to paint.
    ++threadsRequest.current;
    // SearchField already debounces typing before committing `query`.
    const timeout = window.setTimeout(() => {
      void loadThreads(query).finally(() => setLoading(false));
    }, 0);
    return () => window.clearTimeout(timeout);
  }, [query, loadThreads]);

  useEffect(() => {
    // The toggle itself is only reachable while searching; reset it with the
    // search box so the next search starts from all mail again.
    if (!query.trim()) setInboxOnly(false);
  }, [query]);

  useEffect(() => {
    if (!includeArchived) cancelRemoteSearch();
  }, [includeArchived, cancelRemoteSearch]);

  useEffect(() => () => {
    if (remoteSearchTimerRef.current !== null) window.clearTimeout(remoteSearchTimerRef.current);
  }, []);

  return {
    threads, setThreads, query, setQuery, queryRef, searchOpen, setSearchOpen, includeArchived,
    inboxOnly, setInboxOnly, remoteSearchState, searchStatus, setSearchStatus, hasMoreResults, loading,
    mailboxError, loadingMoreState,
    loadThreads, loadThreadsRef, loadMoreResults, contextOpenedThreadRef,
  };
}
