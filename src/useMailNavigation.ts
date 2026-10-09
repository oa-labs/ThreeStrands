import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
  type RefObject,
} from "react";
import type { MailboxKind } from "./commands";
import type { Thread, ThreadDetail } from "./domain";
import { MAILBOX_TITLES } from "./mailboxTitles";
import {
  readSelectedMailboxForAccount,
  readSelectedTabForAccount,
  saveSelectedMailboxForAccount,
  saveSelectedTabForAccount,
} from "./settings";
import {
  captureReaderAnchor,
  currentReturnStep,
  pushReturnStep,
  restoreReaderAnchor,
  settleReturnSteps,
  type ReaderAnchor,
  type ReturnPoint,
  type ReturnStep,
} from "./returnNavigation";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import type { useMailboxThreads } from "./useMailboxThreads";
import type { useThreadDetail } from "./useThreadDetail";
import type { useReaderState } from "./useReaderState";
import type { useWorkspaces } from "./useWorkspaces";
import type { useCorrespondence } from "./useCorrespondence";
import type { useSplitInboxes } from "./useSplitInboxes";
import type { Notice } from "./useNotice";

type Options = {
  mailboxThreads: Pick<ReturnType<typeof useMailboxThreads>, "threads" | "query" | "setQuery" | "searchOpen" | "setSearchOpen" | "contextOpenedThreadRef">;
  threadDetail: Pick<ReturnType<typeof useThreadDetail>, "visibleDetail" | "setDetail" | "setDetailLoading">;
  reader: Pick<ReturnType<typeof useReaderState>, "messageExpansionOverrides" | "setMessageExpansionOverrides" | "messageStackRef" | "messageRefs">;
  workspaces: Pick<ReturnType<typeof useWorkspaces>, "rightWorkspace" | "setRightWorkspace">;
  correspondence: Pick<ReturnType<typeof useCorrespondence>, "context">;
  splitInboxCatalog: Pick<ReturnType<typeof useSplitInboxes>, "splitInboxes" | "loaded">;
  mailbox: MailboxKind;
  setMailbox: Dispatch<SetStateAction<MailboxKind>>;
  activeSplitInboxId: string | null;
  setActiveSplitInboxId: Dispatch<SetStateAction<string | null>>;
  activeAccountId: string | null;
  setActiveAccountId: Dispatch<SetStateAction<string | null>>;
  selectedId: string | null;
  setSelectedId: Dispatch<SetStateAction<string | null>>;
  selectedThreadRowRef: RefObject<HTMLButtonElement | null>;
  setNotice: (notice: Notice | null) => void;
};

/** Coordinates mailbox/account switches and reversible jumps into conversations. */
export function useMailNavigation({
  mailboxThreads, threadDetail, reader, workspaces, correspondence, splitInboxCatalog, mailbox,
  setMailbox, activeSplitInboxId, setActiveSplitInboxId, activeAccountId, setActiveAccountId,
  selectedId, setSelectedId, selectedThreadRowRef, setNotice,
}: Options) {
  const { threads, query, setQuery, searchOpen, setSearchOpen, contextOpenedThreadRef } = mailboxThreads;
  const { visibleDetail, setDetail, setDetailLoading } = threadDetail;
  const { messageExpansionOverrides, setMessageExpansionOverrides, messageStackRef, messageRefs } = reader;
  const { rightWorkspace, setRightWorkspace } = workspaces;
  const { splitInboxes, loaded: splitInboxesLoaded } = splitInboxCatalog;
  // A split inbox belongs to one account, so the tab bar (and Tab/Shift+Tab
  // cycling) only ever offers the active account's own splits — never a
  // different account's, and none at all in the merged "All accounts" view.
  const accountSplitInboxes = useMemo(
    () => splitInboxes.filter((splitInbox) => splitInbox.accountId === activeAccountId),
    [splitInboxes, activeAccountId],
  );
  const activeSplitInbox = activeSplitInboxId
    ? accountSplitInboxes.find((candidate) => candidate.id === activeSplitInboxId) ?? null
    : null;
  const mailboxTitle = mailbox === "split" ? activeSplitInbox?.name ?? MAILBOX_TITLES.split : MAILBOX_TITLES[mailbox];
  useEffect(() => {
    // Deleting the split inbox currently being viewed (e.g. from Settings in
    // another render), or switching to an account it doesn't belong to,
    // shouldn't leave the thread list stuck on a rule that doesn't apply.
    if (activeSplitInboxId && splitInboxesLoaded && !activeSplitInbox) {
      setMailbox("inbox");
      setActiveSplitInboxId(null);
    }
  }, [activeSplitInboxId, activeSplitInbox, splitInboxesLoaded, setMailbox, setActiveSplitInboxId]);
  const restoredTabAccountRef = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    // Restores whichever Inbox/split tab this account last had selected —
    // waits for splitInboxesLoaded so a stored split id isn't mistaken for
    // deleted before the real list has a chance to arrive.
    if (!splitInboxesLoaded || restoredTabAccountRef.current === activeAccountId) return;
    const isAccountChange = restoredTabAccountRef.current !== undefined;
    restoredTabAccountRef.current = activeAccountId;
    // Search is scoped to a single account, so switching accounts shouldn't
    // carry over an open search box or its query (unlike switching between
    // Inbox/split tabs within the same account, which should preserve it).
    if (isAccountChange) {
      setQuery("");
      setSearchOpen(false);
    }
    const stored = readSelectedTabForAccount(activeAccountId);
    if (stored === undefined) return;
    const target = stored && accountSplitInboxes.some((candidate) => candidate.id === stored) ? stored : null;
    setMailbox((current) => (current === "inbox" || current === "split" ? (target ? "split" : "inbox") : current));
    setActiveSplitInboxId(target);
  }, [activeAccountId, splitInboxesLoaded, accountSplitInboxes, setMailbox, setActiveSplitInboxId, setQuery, setSearchOpen]);
  // Jumps to another conversation (from the context panel, Tasks, or Contacts) that Back can undo.
  const [returnSteps, setReturnSteps] = useState<readonly ReturnStep[]>([]);
  const pendingReaderRestoreRef = useRef<{ threadId: string; expanded: [string, boolean][]; anchor: ReaderAnchor | null; stage: "expand" | "scroll" } | null>(null);
  /** Where the user is now, captured before a jump so Back can return to it. */
  const captureReturnPoint = (): ReturnPoint => {
    const workspaceLabel = rightWorkspace === "tasks" ? "Tasks" : rightWorkspace === "contacts" ? "Contacts" : rightWorkspace === "week" ? "Calendar" : null;
    const searchLabel = searchOpen && query.trim() ? `search “${query.trim()}”` : null;
    return {
      accountId: activeAccountId,
      mailbox,
      splitInboxId: activeSplitInboxId,
      query,
      searchOpen,
      workspace: rightWorkspace,
      threadId: selectedId,
      label: workspaceLabel ?? (visibleDetail ? visibleDetail.thread.subject || "(no subject)" : searchLabel ?? mailboxTitle),
      reader: visibleDetail ? {
        expanded: [...messageExpansionOverrides],
        anchor: captureReaderAnchor(messageStackRef.current, messageRefs.current),
      } : null,
    };
  };
  const captureReturnPointRef = useRef(captureReturnPoint);
  captureReturnPointRef.current = captureReturnPoint;

  const openTaskThread = useCallback((threadId: string, origin?: ReturnPoint) => {
    const openThread = (thread: Thread, loadedDetail?: ThreadDetail) => {
      // Recorded only once the conversation opens, so a failed jump leaves nothing to undo.
      if (origin) setReturnSteps((steps) => pushReturnStep(steps, { origin, target: thread.id }));
      const targetMailbox: MailboxKind = thread.trashed ? "trash" : thread.archived ? "allMail" : "inbox";
      const targetAccountId = activeAccountId === null ? null : thread.accountId;
      if (targetAccountId !== activeAccountId) {
        saveSelectedMailboxForAccount(activeAccountId, mailbox === "split" ? "inbox" : mailbox);
        setActiveAccountId(targetAccountId);
      }
      setRightWorkspace(null);
      correspondence.context.openInbox();
      setQuery("");
      setSearchOpen(false);
      setMailbox(targetMailbox);
      setActiveSplitInboxId(null);
      saveSelectedMailboxForAccount(targetAccountId, targetMailbox);
      contextOpenedThreadRef.current = thread.id;
      setSelectedId(thread.id);
      if (loadedDetail) setDetail(loadedDetail);
    };

    const existing = threads.find((thread) => thread.id === threadId);
    if (existing) {
      openThread(existing);
      return;
    }

    contextOpenedThreadRef.current = threadId;
    setDetailLoading(true);
    void mailClient.getThread(threadId)
      .then((thread) => openThread(thread.thread, thread))
      .catch((reason: unknown) => {
        if (contextOpenedThreadRef.current === threadId) contextOpenedThreadRef.current = null;
        setNotice({ message: errorMessage(reason) });
      })
      .finally(() => setDetailLoading(false));
  }, [
    activeAccountId, contextOpenedThreadRef, correspondence.context, mailbox, setActiveAccountId,
    setActiveSplitInboxId, setDetail, setDetailLoading, setMailbox, setNotice, setQuery,
    setRightWorkspace, setSearchOpen, setSelectedId, threads,
  ]);
  /** Opens a conversation from elsewhere in the app, remembering where the user was. */
  const jumpToThread = useCallback((threadId: string) => {
    openTaskThread(threadId, captureReturnPointRef.current());
  }, [openTaskThread]);

  useEffect(() => {
    setReturnSteps((steps) => settleReturnSteps(steps, selectedId));
  }, [selectedId]);
  const returnStep = currentReturnStep(returnSteps, selectedId);
  const goBack = useCallback(() => {
    const step = currentReturnStep(returnSteps, selectedId);
    if (!step) return;
    const origin = step.origin;
    setReturnSteps((steps) => steps.slice(0, -1));
    // Keeps the origin conversation selected while its list reloads, as a jump does for its target.
    contextOpenedThreadRef.current = origin.threadId;
    if (origin.accountId !== activeAccountId) {
      saveSelectedMailboxForAccount(activeAccountId, mailbox === "split" ? "inbox" : mailbox);
      // The origin's own tab and search come back below, so the per-account tab restore must not replace them.
      restoredTabAccountRef.current = origin.accountId;
      setActiveAccountId(origin.accountId);
    }
    setRightWorkspace(origin.workspace);
    if (origin.mailbox === "drafts") correspondence.context.openDrafts();
    else if (origin.mailbox === "outbox") correspondence.context.openOutbox();
    else correspondence.context.openInbox();
    setMailbox(origin.mailbox);
    setActiveSplitInboxId(origin.splitInboxId);
    saveSelectedMailboxForAccount(origin.accountId, origin.mailbox === "split" ? "inbox" : origin.mailbox);
    setQuery(origin.query);
    setSearchOpen(origin.searchOpen);
    setSelectedId(origin.threadId);
    pendingReaderRestoreRef.current = origin.threadId && origin.reader
      ? { threadId: origin.threadId, ...origin.reader, stage: "expand" }
      : null;
    if (origin.workspace === null) window.requestAnimationFrame(() => selectedThreadRowRef.current?.focus({ preventScroll: true }));
  }, [
    activeAccountId, contextOpenedThreadRef, correspondence.context, mailbox, returnSteps,
    selectedId, selectedThreadRowRef, setActiveAccountId, setActiveSplitInboxId, setMailbox,
    setQuery, setRightWorkspace, setSearchOpen, setSelectedId,
  ]);
  // Going back restores the conversation's expanded messages, then (after they lay out) its scroll position.
  useEffect(() => {
    const pending = pendingReaderRestoreRef.current;
    if (!pending || pending.stage !== "expand" || visibleDetail?.thread.id !== pending.threadId) return;
    pending.stage = "scroll";
    setMessageExpansionOverrides((current) => new Map([...current, ...pending.expanded]));
  }, [visibleDetail, setMessageExpansionOverrides]);
  useLayoutEffect(() => {
    const pending = pendingReaderRestoreRef.current;
    if (!pending || pending.stage !== "scroll" || visibleDetail?.thread.id !== pending.threadId) return;
    pendingReaderRestoreRef.current = null;
    if (pending.anchor) restoreReaderAnchor(messageStackRef.current, messageRefs.current, pending.anchor);
  }, [messageExpansionOverrides, visibleDetail, messageRefs, messageStackRef]);

  // Switches to the Inbox tab (`null`) or a split inbox tab and remembers the
  // choice for the active account.
  const goToTab = useCallback((splitInboxId: string | null) => {
    contextOpenedThreadRef.current = null;
    setRightWorkspace(null);
    correspondence.context.openInbox();
    setMailbox(splitInboxId ? "split" : "inbox");
    setActiveSplitInboxId(splitInboxId);
    saveSelectedTabForAccount(activeAccountId, splitInboxId);
    saveSelectedMailboxForAccount(activeAccountId, "inbox");
  }, [contextOpenedThreadRef, setRightWorkspace, correspondence.context, setMailbox, setActiveSplitInboxId, activeAccountId]);
  const goToInboxTab = useCallback(() => goToTab(null), [goToTab]);

  const openMailView = useCallback(() => {
    goToTab(mailbox === "split" ? activeSplitInboxId : null);
  }, [goToTab, mailbox, activeSplitInboxId]);

  const goToSplitTab = useCallback((id: string) => goToTab(id), [goToTab]);

  // Cycles through Inbox + every split inbox tab, in the order the tab bar
  // shows them, wrapping around at either end.
  const goToRelativeSplitTab = useCallback((direction: 1 | -1) => {
    const tabs: (string | null)[] = [null, ...accountSplitInboxes.map((splitInbox) => splitInbox.id)];
    const currentIndex = mailbox === "split" ? tabs.indexOf(activeSplitInboxId) : 0;
    const from = currentIndex === -1 ? 0 : currentIndex;
    goToTab(tabs[(from + direction + tabs.length) % tabs.length] ?? null);
  }, [accountSplitInboxes, mailbox, activeSplitInboxId, goToTab]);
  const goToNextSplitTab = useCallback(() => goToRelativeSplitTab(1), [goToRelativeSplitTab]);
  // Folders outside the Inbox/split tab bar. Each starts with a cleared
  // search; Drafts and Outbox list local items rather than threads, so they
  // also drop the open conversation.
  const openFolder = useCallback((folder: "allMail" | "trash" | "drafts" | "outbox") => {
    contextOpenedThreadRef.current = null;
    setRightWorkspace(null);
    if (folder === "drafts") correspondence.context.openDrafts();
    else if (folder === "outbox") correspondence.context.openOutbox();
    else correspondence.context.openInbox();
    setQuery("");
    setSearchOpen(false);
    setMailbox(folder);
    saveSelectedMailboxForAccount(activeAccountId, folder);
    if (folder === "drafts" || folder === "outbox") {
      setSelectedId(null);
      setDetail(null);
    }
  }, [
    contextOpenedThreadRef, setRightWorkspace, correspondence.context, setQuery, setSearchOpen,
    setMailbox, activeAccountId, setSelectedId, setDetail,
  ]);
  const switchAccount = useCallback((accountId: string | null) => {
    if (accountId === activeAccountId) return;
    contextOpenedThreadRef.current = null;
    const currentMailbox = mailbox === "split" ? "inbox" : mailbox;
    saveSelectedMailboxForAccount(activeAccountId, currentMailbox);
    const nextMailbox = readSelectedMailboxForAccount(accountId) ?? "inbox";
    setMailbox(nextMailbox);
    setActiveSplitInboxId(null);
    setQuery("");
    setSearchOpen(false);
    if (nextMailbox === "drafts") correspondence.context.openDrafts();
    else if (nextMailbox === "outbox") correspondence.context.openOutbox();
    else correspondence.context.openInbox();
    if (nextMailbox === "drafts" || nextMailbox === "outbox") {
      setSelectedId(null);
      setDetail(null);
    }
    setActiveAccountId(accountId);
  }, [
    activeAccountId, contextOpenedThreadRef, correspondence.context, mailbox, setActiveAccountId,
    setActiveSplitInboxId, setDetail, setMailbox, setQuery, setSearchOpen, setSelectedId,
  ]);
  const goToPreviousSplitTab = useCallback(() => goToRelativeSplitTab(-1), [goToRelativeSplitTab]);
  return {
    accountSplitInboxes, mailboxTitle, returnStep, goBack, openTaskThread,
    jumpToThread, goToInboxTab, openMailView, goToSplitTab, goToNextSplitTab, goToPreviousSplitTab,
    openFolder, switchAccount,
  };
}
