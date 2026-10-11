import {
  Archive,
  ArrowLeft,
  CalendarDays,
  CircleAlert,
  Command as CommandIcon,
  ContactRound,
  Inbox,
  Mail,
  MailOpen,
  Moon,
  Pencil,
  RefreshCw,
  RotateCcw,
  Settings as SettingsIcon,
  ShieldAlert,
  SquareCheckBig,
  Star,
  Sun,
  Tag,
  Trash,
  Unlink,
  X,
} from "lucide-react";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { AccountSwitcher } from "./AccountSwitcher";
import { ActionButton, CommandPalette, FiltersButton, HoverTooltip, ShortcutHelp } from "./AppChrome";
import { CalendarSidebar } from "./CalendarSidebar";
import { eventDate, startOfLocalDay } from "./calendarTime";
import { chatAttachmentOptions } from "./chatAttachments";
import { labelCommand, type MailboxKind } from "./commands";
import { draftRecipients } from "./composeChecks";
import { ComposeContext, ReplyChecks } from "./ComposeContext";
import { DraftReviewSection } from "./DraftReviewSection";
import { ContactCardContext } from "./ContactCard";
import { ContactMeetings } from "./ContactMeetings";
import { ContextPanel } from "./ContextPanel";
import { focusContextPanel, handleContextPanelKeyDown } from "./contextPanelFocus";
import { CreateCalendarEventDialog } from "./CreateCalendarEventDialog";
import { mailClient } from "./data/client";
import type { ActionProposal } from "./domain";
import { EnrollmentRequestNotice } from "./EnrollmentRequestNotice";
import { errorMessage, logBackgroundFailure } from "./errors";
import { FolderSwitcher } from "./FolderSwitcher";
import { ICON_SIZE } from "./iconSizes";
import { ImageLightbox } from "./ImageLightbox";
import { LabelManager } from "./LabelManager";
import { conversationLabelGroups, formatLabelName } from "./labels";
import { MeetingProposalDialog } from "./MeetingProposalDialog";
import { MeetingScheduler } from "./MeetingScheduler";
import { MessageCard } from "./MessageCard";
import { OpenedCalendarDialog } from "./OpenedCalendarDialog";
import { PanelResizeHandle, useInboxWidth } from "./PanelResizeHandle";
import { messagesWithQueuedReplies } from "./queuedReplies";
import { AvailabilitySection } from "./RecipientSections";
import { planChatAvailability } from "./scheduling";
import { SearchField } from "./SearchField";
import { readSelectedAccountId, readSelectedMailboxForAccount } from "./settings";
import type { SettingsSection } from "./settingsPanelTypes";
import { readSettingsSection, saveSettingsSection } from "./settingsNavigation";
import { TaskEditorDialog } from "./TaskEditorDialog";
import { TaskSidebar } from "./TaskSidebar";
import { THREAD_ASSIST_ID, ThreadAssist } from "./ThreadAssist";
import { sharedChatAttachments, ThreadChat } from "./ThreadChat";
import { ThreadRow } from "./ThreadList";
import { ThreadTasks } from "./ThreadTasks";
import { threadTextIndex } from "./threadTextIndex";
import { UnsubscribeConfirm } from "./UnsubscribeConfirm";
import { UpdateNotice } from "./UpdateNotice";
import { useAccounts } from "./useAccounts";
import { useAiAvailability } from "./useAiAvailability";
import { useAppCommandContext } from "./useAppCommandContext";
import { useAppPreferences } from "./useAppPreferences";
import { useCommandExecution, useCommandUndo } from "./useCommandExecution";
import { DraftsList, OutboxList, useCorrespondence } from "./useCorrespondence";
import { useLabelCatalog } from "./useLabelCatalog";
import { useMailboxThreads } from "./useMailboxThreads";
import { useMailNavigation } from "./useMailNavigation";
import { useMailRefresh, useUnreadCounts } from "./useMailRefresh";
import { useMailStateActions } from "./useMailStateActions";
import { useMeetingActions } from "./useMeetingActions";
import { useNativeMailLinks } from "./useNativeMailLinks";
import { useNotice } from "./useNotice";
import { useReaderActions } from "./useReaderActions";
import { useReaderState } from "./useReaderState";
import { useSettingsIntegration } from "./useSettingsIntegration";
import { useSnippets } from "./useSnippets";
import { useSplitInboxes } from "./useSplitInboxes";
import { useTaskActions } from "./useTaskActions";
import { useTaskIndicators } from "./useTaskIndicators";
import { useThreadDetail } from "./useThreadDetail";
import { useThreadIntelligence } from "./useThreadIntelligence";
import { useThreadMutations } from "./useThreadMutations";
import { useThreadSelection } from "./useThreadSelection";
import { useTriageSession } from "./useTriageSession";
import { useWorkspaces } from "./useWorkspaces";
export { AccountSwitcher } from "./AccountSwitcher";
export { messagesWithQueuedReplies } from "./queuedReplies";
export { NOTICE_TIMEOUT_MS } from "./useNotice";
export { formatMailTimestamp } from "./threadPresentation";

// Rarely opened workspaces load on demand so they stay off the startup bundle.
const Settings = lazy(() => import("./SettingsPanel").then((m) => ({ default: m.Settings })));
const CalendarWeekView = lazy(() => import("./CalendarWeekView").then((m) => ({ default: m.CalendarWeekView })));
const ContactsWorkspace = lazy(() => import("./ContactsWorkspace").then((m) => ({ default: m.ContactsWorkspace })));

export function App() {
  const inboxSize = useInboxWidth();
  const preferences = useAppPreferences();
  const {
    effectiveTheme: effectiveThemeValue,
    toggleTheme,
    fontScale,
    fontFamily,
    adjustFontScale,
    emailMinimumFontSize,
    autoReadDelaySeconds,
    loadRemoteImages,
    availabilityPreferences,
  } = preferences;
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>(readSettingsSection);
  useEffect(() => saveSettingsSection(settingsSection), [settingsSection]);
  const openSettingsAt = useCallback((section?: SettingsSection) => {
    if (section) setSettingsSection(section);
    setSettingsOpen(true);
  }, []);

  const mailAccounts = useAccounts(settingsOpen);
  const {
    accounts,
    syncStatus,
    setSyncStatus,
    activeAccountId,
    setActiveAccountId,
    mailboxUnreadCounts,
    refreshMailboxUnreadCounts,
    reorderAccounts,
  } = mailAccounts;
  const { unreadCounts, checkingMail, refreshUnreadCounts } = useUnreadCounts(syncStatus, setSyncStatus);
  const [activeSplitInboxId, setActiveSplitInboxId] = useState<string | null>(null);
  const [mailbox, setMailbox] = useState<MailboxKind>(() => readSelectedMailboxForAccount(readSelectedAccountId()) ?? "inbox");
  const mailboxThreads = useMailboxThreads({
    activeAccountId, mailbox, activeSplitInboxId, setSelectedId, refreshUnreadCounts,
    refreshMailboxUnreadCounts,
  });
  const {
    threads, setThreads, query, setQuery, queryRef, searchOpen, setSearchOpen, includeArchived,
    setIncludeArchived, remoteSearchState, hasMoreResults, loading, mailboxError, loadingMoreState,
    loadThreads, loadThreadsRef, loadMoreResults, contextOpenedThreadRef,
  } = mailboxThreads;
  const selection = useThreadSelection({ threads, selectedId, setSelectedId, query, includeArchived, contextOpenedThreadRef });
  const {
    checkedIds, setCheckedIds, selectedThreadRowRef, selectAllRef, activeMessageFilters,
    toggleMessageFilter, visibleThreads, selected, selectThread, applyThreadSelectionGesture,
    toggleChecked,
  } = selection;
  const [notice, setNotice] = useNotice();
  const threadDetail = useThreadDetail(selectedId, threads, setNotice);
  const { detail, setDetail, visibleDetail, detailLoading } = threadDetail;
  const { applyThreadSummary } = useMailStateActions({ mailboxThreads, threadDetail });
  const snippetLibrary = useSnippets();
  const correspondence = useCorrespondence(
    accounts, visibleDetail?.messages.at(-1)?.id, visibleDetail?.thread.accountId,
    snippetLibrary.snippets, snippetLibrary.create, snippetLibrary.update, snippetLibrary.remove,
    selectedId,
    { theme: effectiveThemeValue, fontScale: fontScale / 100, fontFamily, emailMinimumFontSize, loadImages: loadRemoteImages },
  );
  const composerBelongsToVisibleThread = Boolean(
    correspondence.activeDraft
    && correspondence.activeDraft.mode !== "new"
    && visibleDetail?.messages.some((message) => message.id === correspondence.activeDraft?.sourceId),
  );
  // A new message, a forward, or a draft opened from the list: the reader
  // shows only the composer, so the context panel follows the draft instead.
  const composingApart = Boolean(correspondence.activeDraft && !composerBelongsToVisibleThread);
  // Who a reply in the open conversation goes to; the context panel follows them.
  const replyRecipients = useMemo(
    () => composerBelongsToVisibleThread && correspondence.liveDraft
      ? draftRecipients(correspondence.liveDraft, accounts.map((account) => account.email))
      : [],
    [accounts, composerBelongsToVisibleThread, correspondence.liveDraft],
  );
  const displayedMessages = useMemo(
    () => visibleDetail ? messagesWithQueuedReplies(visibleDetail, correspondence.outbox) : [],
    [visibleDetail, correspondence.outbox],
  );
  const displayedThreadText = useMemo(() => threadTextIndex(displayedMessages), [displayedMessages]);
  const reader = useReaderState({
    selectedThreadId: selectedId,
    detail: visibleDetail,
    displayedMessages,
    composerOpen: composerBelongsToVisibleThread,
  });
  const {
    messageExpansionOverrides,
    messageStackRef,
    activeMessageId,
    latestDisplayedMessageId,
  } = reader;
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [shortcutHelpOpen, setShortcutHelpOpen] = useState(false);
  const [unsubscribeMessageId, setUnsubscribeMessageId] = useState<string | null>(null);
  const workspaces = useWorkspaces({ settingsOpen, settingsSection, visibleDetail, openSettingsAt, setNotice });
  const {
    rightWorkspace, setRightWorkspace, calendarWeekAnchor, setCalendarWeekAnchor,
    calendarEventToOpen, openCalendarAt, contactAddressBookTarget, contactsView, setContactsView,
    keepInTouchDueCount, refreshKeepInTouchCount, calendar, refreshCalendarOptions,
    calendarConnected, calendarSidebarStart, openContactsView, contactCardActions,
    contextPersonEmail, openCalendarView,
  } = workspaces;
  const { openTaskThreadIds, taskRevision, setTaskRevision, refreshTaskIndicators } = useTaskIndicators(activeAccountId);
  const splitInboxCatalog = useSplitInboxes();
  const { refresh: refreshSplitInboxes } = splitInboxCatalog;
  const isThreadMailbox = mailbox === "inbox" || mailbox === "allMail" || mailbox === "trash" || mailbox === "split";
  const isTabbedMailbox = mailbox === "inbox" || mailbox === "split";
  const navigation = useMailNavigation({
    mailboxThreads, threadDetail, reader, workspaces, correspondence, splitInboxCatalog, mailbox,
    setMailbox, activeSplitInboxId, setActiveSplitInboxId, activeAccountId, setActiveAccountId,
    selectedId, setSelectedId, selectedThreadRowRef, setNotice,
  });
  const { accountSplitInboxes, mailboxTitle, returnStep, openTaskThread, jumpToThread } = navigation;
  const { refreshMail, recoveryStatus, syncDiagnostics } = useMailRefresh({ query, loadThreadsRef, setSyncStatus, refreshTaskIndicators });
  const [labelTargetIds, setLabelTargetIds] = useState<string[] | null>(null);
  // Label ids belong to an account. Bulk actions target one account, so the
  // first target thread determines which catalog the label manager shows.
  const labelTargetAccountId = labelTargetIds?.length
    ? threads.find((thread) => thread.id === labelTargetIds[0])?.accountId ?? activeAccountId ?? undefined
    : undefined;
  const [lightboxImageSrc, setLightboxImageSrc] = useState<string | null>(null);
  const {
    aiSummaryAvailable, aiSummaryFeatureEnabled, aiProactive, aiActionFeatureEnabled,
    aiActionAvailable, aiChatFeatureEnabled, aiChatAvailable, aiDraftAvailable, refreshAiAvailability,
  } = useAiAvailability(settingsOpen);
  const {
    mailAccountSettings, applyImportedSettings,
  } = useSettingsIntegration({ mailAccounts, preferences, mailboxThreads, refreshSplitInboxes, refreshAiAvailability, setSettingsSection });
  const {
    labelsByAccount, createLabel, deleteLabel, renameLabel,
  } = useLabelCatalog(accounts, detail?.thread.accountId, labelTargetAccountId, () => loadThreads(query));
  const undo = useCommandUndo(setNotice);
  const { rememberUndo } = undo;
  const searchRef = useRef<HTMLInputElement>(null);
  useNativeMailLinks(correspondence.composeMailto, setRightWorkspace);

  // Reload only when a send completes. Outbox history persists, so testing
  // `sentCount > 0` would stay true forever and turn every search keystroke
  // into an undebounced reload.
  const previousSentCountRef = useRef(correspondence.sentCount);
  useEffect(() => {
    const previous = previousSentCountRef.current;
    previousSentCountRef.current = correspondence.sentCount;
    if (correspondence.sentCount > previous) void loadThreadsRef.current(queryRef.current);
  }, [correspondence.sentCount, loadThreadsRef, queryRef]);

  useEffect(() => {
    if (searchOpen && isTabbedMailbox) searchRef.current?.focus();
  }, [isTabbedMailbox, searchOpen]);

  const { triageSessionRef, recordTriageEvent } = useTriageSession({ visibleDetail, mailbox, includeArchived, messageStackRef });
  const mutateIds = useThreadMutations({
    threads, detail, visibleDetail, selectedId, setSelectedId, setDetail, setThreads,
    setCheckedIds, setSyncStatus, setNotice, mailbox, activeAccountId, activeSplitInboxId,
    includeArchived, isThreadMailbox, autoReadDelaySeconds, triageSessionRef, recordTriageEvent,
    loadThreadsRef, queryRef,
  });

  const accountColors = useMemo(
    () => new Map(accounts.map((account) => [account.email, account.color] as const)),
    [accounts],
  );
  const latestMessage = visibleDetail?.messages.at(-1) ?? null;
  const canUnsubscribe = Boolean(latestMessage?.unsubscribe?.methods.length);
  const unsubscribeMessage = visibleDetail?.messages.find((message) => message.id === unsubscribeMessageId) ?? null;
  const intelligence = useThreadIntelligence({
    accounts, selected, visibleDetail, isThreadMailbox, autoReadDelaySeconds,
    availabilityPreferences, applyThreadSummary, aiProactive, aiSummaryAvailable,
    aiActionAvailable, aiActionFeatureEnabled,
  });
  const {
    summaryPending, summaryError, runBrief, actionProposals, actionProposalSource,
    actionAnalysisLoading, actionAnalysisError, actionAnalysisRequested, actionAnalysisPreview,
    actionHiddenCount, chatByThread, chatPendingThreads, chatFailures, chatFocusRequest, askThread,
    focusThreadChat, updateActionProposal, removeActionProposal, discardActionProposal,
    ownAddresses,
  } = intelligence;
  const tasks = useTaskActions({
    correspondence, setSelectedId, setDetail, accounts, activeAccountId, rightWorkspace,
    visibleDetail, selectedId, setNotice, setTaskRevision, refreshTaskIndicators,
    updateActionProposal, removeActionProposal,
  });
  const {
    draftFollowUp, taskWorkspaceRef, setSelectedTaskStatus, setTaskLayout,
    setSelectedTaskHasThread, taskEditor, setTaskEditor, taskEditorAccountId, taskEditorGoals,
    submitTaskEditor, newTask, createWorkspaceTask,
  } = tasks;
  const {
    meetingEditor, setMeetingEditor, meetingEventDraft, setMeetingEventDraft, openedCalendarFiles,
    dismissOpenedCalendarFile, addOpenedInvitationToCalendar, addMeetingToCalendar,
    addComposeMeeting, meetingCreated, meetingScheduling, draftAvailabilityReply,
  } = useMeetingActions({ workspaces, correspondence, visibleDetail, availabilityPreferences, openSettingsAt, setNotice, ownAddresses, removeActionProposal });
  const { systemLabelNames: conversationSystemLabels, userLabels: conversationUserLabels } = visibleDetail
    ? conversationLabelGroups(visibleDetail.thread.labels, labelsByAccount[visibleDetail.thread.accountId])
    : { systemLabelNames: [], userLabels: [] };

  const confirmUnsubscribe = useCallback(async () => {
    if (!unsubscribeMessageId) return;
    try {
      const result = await mailClient.unsubscribe(unsubscribeMessageId);
      setUnsubscribeMessageId(null);
      setNotice({
        message: result.outcome === "requested"
          ? "Unsubscribe request sent"
          : "Opened unsubscribe option",
      });
    } catch (reason: unknown) {
      setNotice({
        message: `Unsubscribe failed: ${errorMessage(reason)}`,
      });
    }
  }, [setNotice, unsubscribeMessageId]);

  /** Shows the context panel and moves focus into its question box. */
  const openThreadChat = useCallback(() => {
    setRightWorkspace((current) => current === "calendar" ? current : null);
    focusThreadChat();
  }, [focusThreadChat, setRightWorkspace]);

  /** Shows the context panel's AI section and fetches suggestions if missing. */
  const getSuggestions = useCallback(() => {
    setRightWorkspace((current) => current === "calendar" ? current : null);
    window.requestAnimationFrame(() => document.getElementById(THREAD_ASSIST_ID)?.scrollIntoView?.({ block: "nearest" }));
    void runBrief({ only: "suggestions" });
  }, [runBrief, setRightWorkspace]);

  const reviewActionProposal = useCallback((index: number, proposal: ActionProposal, intent: "edit" | "accept") => {
    if (!visibleDetail || !actionProposalSource) return;
    if (proposal.type === "task") {
      setTaskEditor({ kind: "proposal", thread: visibleDetail, source: actionProposalSource, index, proposal, intent });
    } else {
      setMeetingEditor({ index, proposal });
    }
  }, [actionProposalSource, setMeetingEditor, setTaskEditor, visibleDetail]);

  // Opening a conversation replaces the composer, so save the draft first; it stays in Drafts.
  const openThreadFromDraft = useCallback((threadId: string) => {
    void correspondence.flushDraft()
      .then(() => openTaskThread(threadId))
      .catch((reason: unknown) => setNotice({ message: errorMessage(reason) }));
  }, [correspondence, openTaskThread, setNotice]);
  const {
    selectAdjacentMessage, activateMessage, toggleMessage, showMessage, registerMessageNode,
    respondToMessage,
  } = useReaderActions({ reader, displayedMessages, visibleDetail, selected, correspondence, jumpToThread, mailbox, includeArchived, recordTriageEvent });

  const interactionScope = correspondence.activeDraft
    ? "compose"
    : paletteOpen
      ? "palette"
      : settingsOpen || shortcutHelpOpen || Boolean(unsubscribeMessage) || Boolean(labelTargetIds?.length) || Boolean(taskEditor) || Boolean(meetingEditor)
        ? "modal"
        : "read";

  // F6 / Mod+Shift+P: from the draft into the context panel, and back to the caret.
  const { focusDraftBody, activeDraft } = correspondence;
  const toggleContextPanelFocus = useCallback(() => {
    const panel = document.querySelector<HTMLElement>(".context-panel");
    if (!panel) return;
    if (panel.contains(document.activeElement)) focusDraftBody();
    else focusContextPanel(panel);
  }, [focusDraftBody]);
  const contextPanelKeyDown = useCallback((event: ReactKeyboardEvent<HTMLElement>) => {
    if (activeDraft) handleContextPanelKeyDown(event, focusDraftBody);
  }, [activeDraft, focusDraftBody]);

  const context = useAppCommandContext({
    correspondence, reader, workspaces, navigation, selection, tasks, undo, mailboxThreads,
    runBrief, recordTriageEvent, displayedMessages, composerBelongsToVisibleThread, mutateIds,
    view: { mailbox, setMailbox, setActiveSplitInboxId, activeAccountId, selectedId, setSelectedId, isTabbedMailbox },
    ui: { interactionScope, canUnsubscribe, latestMessage, aiSummaryAvailable, labelTargetIds, setLabelTargetIds, setUnsubscribeMessageId, setPaletteOpen, setShortcutHelpOpen, openSettingsAt, adjustFontScale, toggleContextPanelFocus, selectAdjacentMessage, getSuggestions, openThreadChat, refreshMail },
  });

  const {
    executeCommand, executeById, paletteExtraCommands, runOnSelection,
  } = useCommandExecution({ context, accounts, accountSplitInboxes, checkedIds, mutateIds, rememberUndo, setNotice });

  const reorderNavbarAccounts = useCallback((emails: string[]) => {
    void reorderAccounts(emails).catch(() => {
      setNotice({ message: "Account order could not be saved" });
    });
  }, [reorderAccounts, setNotice]);

  const clearContextOpenedThread = useCallback(() => {
    contextOpenedThreadRef.current = null;
  }, [contextOpenedThreadRef]);
  const closeSearch = useCallback(() => {
    setQuery("");
    setSearchOpen(false);
    selectedThreadRowRef.current?.focus();
  }, [selectedThreadRowRef, setQuery, setSearchOpen]);
  const toggleIncludeArchived = useCallback(() => setIncludeArchived((current) => !current), [setIncludeArchived]);

  const showNoticeMessage = useCallback((message: string) => setNotice({ message }), [setNotice]);

  const selectedThreads = threads.filter((thread) => checkedIds.has(thread.id));
  const allSelectedThreadsStarred = selectedThreads.length > 0
    && selectedThreads.every((thread) => thread.starred);
  const batchStarLabel = allSelectedThreadsStarred ? "Unstar" : "Star";

  return (
    <main className={`app-shell${rightWorkspace === "tasks" ? " tasks-open" : rightWorkspace === "contacts" ? " contacts-open" : rightWorkspace === "week" ? " week-open" : rightWorkspace ? " calendar-open" : ""}${rightWorkspace === "calendar" ? " mail-context-open" : ""}`} style={{ "--inbox-width": `${inboxSize.width}px` } as CSSProperties}>
      <nav className="sidebar" aria-label="Mailboxes">
        <AccountSwitcher
          accounts={accounts}
          unreadCounts={unreadCounts}
          activeAccountId={activeAccountId}
          onSwitch={context.switchAccount}
          onShowAll={context.showAllAccounts}
          onReorder={reorderNavbarAccounts}
        />
        <div className="sidebar-spacer" />
        <div className="sidebar-nav">
          <HoverTooltip title="New message (c)"><button className="nav-button" aria-label="New message (c)" onClick={() => executeById("draft.new")}><Pencil size={ICON_SIZE.lg} /></button></HoverTooltip>
          <HoverTooltip label="Inbox" shortcut="1">
            <button
              className={`nav-button ${rightWorkspace !== "tasks" && rightWorkspace !== "week" && rightWorkspace !== "contacts" ? "active" : ""}`}
              aria-label="Inbox (1)"
              onClick={() => executeById("view.mail")}
            >
              <Inbox size={ICON_SIZE.lg} />
            </button>
          </HoverTooltip>
          <HoverTooltip label="Calendar" shortcut="2">
            <button
              className={`nav-button ${rightWorkspace === "week" ? "active" : ""}`}
              aria-label="Calendar (2)"
              onClick={() => executeById("view.calendar")}
            >
              <CalendarDays size={ICON_SIZE.lg} />
            </button>
          </HoverTooltip>
          <HoverTooltip label="Tasks" shortcut="3">
            <button
              className={`nav-button ${rightWorkspace === "tasks" ? "active" : ""}`}
              aria-label="Tasks (3)"
              onClick={() => executeById("tasks.open")}
            >
              <SquareCheckBig size={ICON_SIZE.lg} />
            </button>
          </HoverTooltip>
          <HoverTooltip label={keepInTouchDueCount ? `Contacts · ${keepInTouchDueCount} to reconnect with` : "Contacts"} shortcut="4">
            <button
              className={`nav-button ${rightWorkspace === "contacts" ? "active" : ""}`}
              aria-label={keepInTouchDueCount ? `Contacts (4), ${keepInTouchDueCount} due to reconnect` : "Contacts (4)"}
              onClick={openContactsView}
            >
              <ContactRound size={ICON_SIZE.lg} />
              {keepInTouchDueCount ? <span className="nav-button-badge" aria-hidden="true">{keepInTouchDueCount > 99 ? "99+" : keepInTouchDueCount}</span> : null}
            </button>
          </HoverTooltip>
          <hr className="sidebar-nav-separator" aria-hidden="true" />
          <HoverTooltip title={checkingMail ? "Checking for mail…" : "Refresh mail"}><button className="nav-button" aria-label="Refresh mail" aria-busy={checkingMail} onClick={() => executeById("mail.refresh")}>
            <RefreshCw size={ICON_SIZE.lg} className={checkingMail ? "spin" : ""} />
          </button></HoverTooltip>
          <HoverTooltip title={`Switch to ${effectiveThemeValue === "dark" ? "light" : "dark"} mode`}>
            <button className="nav-button" aria-label={`Switch to ${effectiveThemeValue === "dark" ? "light" : "dark"} mode`} onClick={toggleTheme}>
              {effectiveThemeValue === "dark" ? <Sun size={ICON_SIZE.lg} /> : <Moon size={ICON_SIZE.lg} />}
            </button>
          </HoverTooltip>
          <HoverTooltip label="Command Palette" shortcut="⌘K">
            <button
              className="nav-button"
              aria-label="Command Palette (⌘K)"
              onClick={() => executeById("palette.open")}
            >
              <CommandIcon size={ICON_SIZE.lg} />
            </button>
          </HoverTooltip>
          <HoverTooltip title="Settings (⌘,)"><button className="nav-button" aria-label="Settings (⌘,)" onClick={() => executeById("settings.open")}>
            <SettingsIcon size={ICON_SIZE.lg} />
          </button></HoverTooltip>
        </div>
      </nav>

      {rightWorkspace !== "tasks" && rightWorkspace !== "week" && rightWorkspace !== "contacts" ? <>
      <section id="inbox-panel" className="thread-column" aria-label="Inbox">
        <PanelResizeHandle {...inboxSize} label="Resize Inbox" controlsId="inbox-panel" title="Drag to resize inbox. Use arrow keys to adjust; double-click to reset." />
        <header className="thread-header">
          {isThreadMailbox && checkedIds.size > 0 ? (
            <div className="batch-toolbar" role="toolbar" aria-label="Batch actions">
              <label className="select-all">
                <input
                  ref={selectAllRef}
                  type="checkbox"
                  checked={threads.length > 0 && checkedIds.size === threads.length}
                  aria-label="Select All Conversations"
                  onChange={(event) =>
                    setCheckedIds(event.target.checked ? new Set(threads.map((thread) => thread.id)) : new Set())
                  }
                />
              </label>
              <span className="batch-count">{checkedIds.size} selected</span>
              {/* Trash swaps Archive/Trash/Spam for Restore: 6 actions -> 3 columns, otherwise 8 -> 4. */}
              <div className="batch-actions" style={{ "--batch-columns": mailbox === "trash" ? 3 : 4 } as CSSProperties}>
                {mailbox === "trash" ? (
                  <HoverTooltip label="Restore" placement="bottom">
                    <ActionButton label="Restore" onClick={() => runOnSelection("Restore", { kind: "trash", value: false })}>
                      <RotateCcw size={ICON_SIZE.md} />
                    </ActionButton>
                  </HoverTooltip>
                ) : (
                  <>
                    <HoverTooltip label="Archive" placement="bottom">
                      <ActionButton label="Archive" onClick={() => runOnSelection("Archive", { kind: "archive", value: true })}>
                        <Archive size={ICON_SIZE.md} />
                      </ActionButton>
                    </HoverTooltip>
                    <HoverTooltip label="Trash" placement="bottom">
                      <ActionButton label="Trash" onClick={() => runOnSelection("Trash", { kind: "trash", value: true })}>
                        <Trash size={ICON_SIZE.md} />
                      </ActionButton>
                    </HoverTooltip>
                    <HoverTooltip label="Mark spam" placement="bottom">
                      <ActionButton label="Mark Spam" onClick={() => runOnSelection("Mark Spam", { kind: "spam", value: true })}>
                        <ShieldAlert size={ICON_SIZE.md} />
                      </ActionButton>
                    </HoverTooltip>
                  </>
                )}
                <HoverTooltip label="Mark read" placement="bottom">
                  <ActionButton label="Mark Read" onClick={() => runOnSelection("Mark Read", { kind: "read", value: true })}>
                    <MailOpen size={ICON_SIZE.md} />
                  </ActionButton>
                </HoverTooltip>
                <HoverTooltip label="Mark unread" placement="bottom">
                  <ActionButton label="Mark Unread" onClick={() => runOnSelection("Mark Unread", { kind: "read", value: false })}>
                    <Mail size={ICON_SIZE.md} />
                  </ActionButton>
                </HoverTooltip>
                <HoverTooltip label={batchStarLabel} placement="bottom">
                  <ActionButton
                    label={batchStarLabel}
                    onClick={() => runOnSelection(batchStarLabel, { kind: "star", value: !allSelectedThreadsStarred })}
                  >
                    <Star size={ICON_SIZE.md} fill={allSelectedThreadsStarred ? "currentColor" : "none"} />
                  </ActionButton>
                </HoverTooltip>
                <HoverTooltip label="Labels" placement="bottom">
                  <ActionButton label="Labels" onClick={() => setLabelTargetIds([...checkedIds])}>
                    <Tag size={ICON_SIZE.md} />
                  </ActionButton>
                </HoverTooltip>
                <HoverTooltip label="Clear selection" placement="bottom">
                  <button
                    className="btn-icon"
                    aria-label="Clear Selection"
                    onClick={() => setCheckedIds(new Set())}
                  >
                    <X size={ICON_SIZE.lg} />
                  </button>
                </HoverTooltip>
              </div>
            </div>
          ) : (
            <>
              <div className="thread-header-title">
                <div>
                  <div className="mailbox-heading-context">
                    <FolderSwitcher
                      selected={isTabbedMailbox ? "inbox" : mailbox}
                      inboxUnreadCount={mailboxUnreadCounts.inbox}
                      draftCount={correspondence.draftCount}
                      outboxCount={correspondence.outboxCount}
                      onSelect={(commandId) => executeById(commandId)}
                    />
                    <span className="eyebrow-account">· {activeAccountId ?? "All accounts"}</span>
                  </div>
                  <h1>
                    {mailbox === "drafts"
                      ? `${correspondence.drafts.length} drafts`
                      : mailbox === "outbox"
                        ? `${correspondence.outbox.filter((item) => item.state !== "canceled").length + correspondence.summaries.filter((item) => item.state !== "canceled").length} outgoing`
                        : `${visibleThreads.length} conversations`}
                  </h1>
                  {isTabbedMailbox ? (
                    <div className="mailbox-tabs">
                      <div className="mailbox-tab-list" role="tablist" aria-label="Mailbox views">
                        <button
                          type="button"
                          role="tab"
                          aria-selected={mailbox === "inbox"}
                          className={`mailbox-tab ${mailbox === "inbox" ? "active" : ""}`}
                          onClick={() => context.openInbox()}
                        >
                          Main{mailboxUnreadCounts.inbox > 0 ? ` ${mailboxUnreadCounts.inbox}` : ""}
                        </button>
                        {accountSplitInboxes.map((splitInbox) => (
                          <button
                            key={splitInbox.id}
                            type="button"
                            role="tab"
                            aria-selected={mailbox === "split" && activeSplitInboxId === splitInbox.id}
                            className={`mailbox-tab ${mailbox === "split" && activeSplitInboxId === splitInbox.id ? "active" : ""}`}
                            onClick={() => context.openSplitInbox(splitInbox.id)}
                          >
                            {splitInbox.name}
                            {mailboxUnreadCounts.splits[splitInbox.id] ? ` ${mailboxUnreadCounts.splits[splitInbox.id]}` : ""}
                          </button>
                        ))}
                      </div>
                      <button type="button" className="mailbox-tab-add" onClick={() => openSettingsAt("splitInboxes")}>
                        + Add Split
                      </button>
                    </div>
                  ) : null}
                </div>
              </div>
              {isThreadMailbox ? (
                <FiltersButton activeFilters={activeMessageFilters} onToggleFilter={toggleMessageFilter} />
              ) : null}
            </>
          )}
        </header>
        {isTabbedMailbox && searchOpen ? (
          <div className="list-toolbar">
            <SearchField
              inputRef={searchRef}
              query={query}
              onCommit={setQuery}
              onInput={clearContextOpenedThread}
              onEscape={closeSearch}
              includeArchived={includeArchived}
              onToggleIncludeArchived={toggleIncludeArchived}
            />
          </div>
        ) : null}
        <div className="thread-list" role={isThreadMailbox ? "listbox" : "list"} aria-label={mailboxTitle}>
          {mailbox === "drafts" ? (
            <DraftsList drafts={correspondence.drafts} onOpen={correspondence.openDraft} onDiscard={correspondence.discardListedDraft} />
          ) : mailbox === "outbox" ? (
            <OutboxList
              outbox={correspondence.outbox}
              summaries={correspondence.summaries}
              onChanged={correspondence.refresh}
              clock={correspondence.clock}
              onUndo={correspondence.undoSendItem}
              onRestore={correspondence.restoreFailedSend}
              onReconcile={correspondence.reconcileSend}
              pendingActions={correspondence.pendingOutboxActions}
            />
          ) : (
            <>
          {loading ? <p className="empty">Loading inbox…</p> : null}
          {mailboxError ? <p className="empty mailbox-error" role="alert">{mailboxError}</p> : null}
          {!loading && threads.length === 0 ? (
            accounts.length === 0 ? (
              <div className="connect-account-cta">
                <Mail size={ICON_SIZE.display} />
                <p>Connect your Gmail account to start syncing mail.</p>
                <button type="button" className="btn btn-primary" onClick={() => openSettingsAt("accounts")}>
                  Add Account
                </button>
              </div>
            ) : query.trim() && remoteSearchState === "searching" ? null : (
              <p className="empty">{query.trim() ? "No conversations match your search." : mailbox === "trash" ? "No trashed messages." : "Inbox zero."}</p>
            )
          ) : null}
          {!loading && threads.length > 0 && visibleThreads.length === 0 ? (
            <p className="empty">No conversations match the selected filters.</p>
          ) : null}
          {visibleThreads.map((thread) => (
            <ThreadRow
              key={thread.id}
              thread={thread}
              selected={thread.id === selectedId}
              checked={checkedIds.has(thread.id)}
              showAccount={accounts.length > 1}
              accountColor={accountColors.get(thread.accountId)}
              onSelect={selectThread}
              onToggleCheck={toggleChecked}
              onSelectionGesture={applyThreadSelectionGesture}
              rowRef={thread.id === selectedId ? selectedThreadRowRef : undefined}
              hasTask={openTaskThreadIds.has(thread.id)}
            />
          ))}
          {hasMoreResults ? (
            <button className="btn load-more" onClick={() => void loadMoreResults()} disabled={loadingMoreState}>
              {loadingMoreState ? "Loading…" : "Load More Results"}
            </button>
          ) : null}
            </>
          )}
        </div>
      </section>

      <section className="reader" aria-label="Conversation">
        {visibleDetail && (!correspondence.activeDraft || composerBelongsToVisibleThread) ? (
          <>
            <header className="reader-header">
              <div>
                {returnStep ? (
                  <HoverTooltip label={`Back to ${returnStep.origin.label}`} shortcut="Esc" placement="bottom">
                    <button type="button" className="btn-link reader-back" onClick={() => executeById("navigation.back")}>
                      <ArrowLeft size={ICON_SIZE.sm} aria-hidden="true" />
                      <span className="reader-back-label">Back to {returnStep.origin.label}</span>
                    </button>
                  </HoverTooltip>
                ) : null}
                <span className="reader-account-scope">{visibleDetail.thread.accountId}</span>
                {conversationSystemLabels.length > 0 ? (
                  <span className="eyebrow">{conversationSystemLabels.join(" · ")}</span>
                ) : null}
                <div className="subject-row">
                  <h2>{visibleDetail.thread.subject}</h2>
                  {conversationUserLabels.length > 0 ? (
                    <span className="user-label-badges">
                      {conversationUserLabels.map((label) => (
                        <span key={label.id} className="user-label-badge">{formatLabelName(label)}</span>
                      ))}
                    </span>
                  ) : null}
                </div>
              </div>
              <div className="reader-actions">
                <HoverTooltip label={selected?.starred ? "Unstar" : "Star"} shortcut="s" placement="bottom">
                  <ActionButton
                    label={selected?.starred ? "Unstar" : "Star"}
                    shortcut="s"
                    onClick={() => executeById("thread.star")}
                  >
                    <Star size={ICON_SIZE.lg} fill={selected?.starred ? "currentColor" : "none"} />
                  </ActionButton>
                </HoverTooltip>
                <HoverTooltip
                  label={selected?.unread ? "Mark read" : "Mark unread"}
                  shortcut="u"
                  placement="bottom"
                >
                  <ActionButton
                    label={selected?.unread ? "Mark Read" : "Mark Unread"}
                    shortcut="u"
                    onClick={() => executeById("thread.read")}
                  >
                    {selected?.unread ? <MailOpen size={ICON_SIZE.lg} /> : <Mail size={ICON_SIZE.lg} />}
                  </ActionButton>
                </HoverTooltip>
                {canUnsubscribe ? (
                  <HoverTooltip label="Unsubscribe" shortcut="⌘U" placement="bottom">
                    <ActionButton label="Unsubscribe" shortcut="⌘U" onClick={() => executeById("thread.unsubscribe")}>
                      <Unlink size={ICON_SIZE.lg} />
                    </ActionButton>
                  </HoverTooltip>
                ) : null}
                <HoverTooltip label="Manage Labels" shortcut="L" placement="bottom">
                  <ActionButton label="Labels" shortcut="l" onClick={() => executeById("labels.open")}>
                    <Tag size={ICON_SIZE.lg} />
                  </ActionButton>
                </HoverTooltip>
                {mailbox === "trash" ? null : selected?.archived ? (
                  <HoverTooltip label="Mark not done" shortcut="Shift+E" placement="bottom">
                    <ActionButton label="Mark Not Done" shortcut="Shift+E" onClick={() => executeById("thread.unarchive")}>
                      <Inbox size={ICON_SIZE.lg} />
                    </ActionButton>
                  </HoverTooltip>
                ) : (
                  <HoverTooltip label="Archive" shortcut="e" placement="bottom">
                    <ActionButton label="Archive" shortcut="e" onClick={() => executeById("thread.archive")}>
                      <Archive size={ICON_SIZE.lg} />
                    </ActionButton>
                  </HoverTooltip>
                )}
                {mailbox === "trash" ? (
                  <HoverTooltip label="Restore" placement="bottom">
                    <ActionButton label="Restore" onClick={() => executeById("thread.untrash")}>
                      <RotateCcw size={ICON_SIZE.lg} />
                    </ActionButton>
                  </HoverTooltip>
                ) : (
                  <HoverTooltip label="Trash" shortcut="#" placement="bottom">
                    <ActionButton label="Trash" shortcut="#" onClick={() => executeById("thread.trash")}>
                      <Trash size={ICON_SIZE.lg} />
                    </ActionButton>
                  </HoverTooltip>
                )}
                <HoverTooltip label="Mark spam" shortcut="!" placement="bottom">
                  <ActionButton label="Mark Spam" shortcut="!" onClick={() => executeById("thread.spam")}>
                    <ShieldAlert size={ICON_SIZE.lg} />
                  </ActionButton>
                </HoverTooltip>
              </div>
            </header>
          </>
        ) : null}
        {/* Keep one message-stack host while a draft is active. A sync can
            change whether a reply still belongs to the visible conversation;
            moving the composer between separate branches would remount its
            browser-owned contenteditable DOM and erase unsaved keystrokes. */}
        {correspondence.activeDraft || visibleDetail ? (
          <ContactCardContext.Provider value={contactCardActions}>
          <div
            className={`message-stack${correspondence.activeDraft && !composerBelongsToVisibleThread ? " draft-message-stack" : ""}`}
            ref={visibleDetail && (!correspondence.activeDraft || composerBelongsToVisibleThread) ? messageStackRef : undefined}
          >
            {visibleDetail && (!correspondence.activeDraft || composerBelongsToVisibleThread) ? displayedMessages.map((message, index) => {
                const isLatest = index === displayedMessages.length - 1;
                return (
                  <MessageCard
                    key={message.id}
                    message={message}
                    index={index}
                    isLatest={isLatest}
                    isExpanded={messageExpansionOverrides.get(message.id) ?? (isLatest || message.unread)}
                    isActive={(activeMessageId ?? latestDisplayedMessageId) === message.id}
                    accounts={accounts}
                    queuedItem={message.id.startsWith("outbox-")
                      ? correspondence.outbox.find((item) => `outbox-${item.id}` === message.id)
                      : undefined}
                    loadRemoteImages={loadRemoteImages}
                    theme={effectiveThemeValue}
                    fontScale={fontScale / 100}
                    fontFamily={fontFamily}
                    emailMinimumFontSize={emailMinimumFontSize}
                    threadText={displayedThreadText}
                    onActivate={activateMessage}
                    onToggle={toggleMessage}
                    onRespond={respondToMessage}
                    onRegisterNode={registerMessageNode}
                    onImageClick={setLightboxImageSrc}
                    onNotice={showNoticeMessage}
                  />
                );
            }) : null}
            {correspondence.activeDraft ? correspondence.composer : null}
          </div>
          </ContactCardContext.Provider>
        ) : detailLoading ? (
          <div className="reader-empty" role="status">
            <p>Loading conversation…</p>
          </div>
        ) : (
          <div className="reader-empty">
            <Mail size={ICON_SIZE.display} />
            <p>
              {mailbox === "drafts"
                ? "Select a draft to open it for editing"
                : mailbox === "outbox"
                  ? "Delivery details are shown in the list"
                  : "Select a conversation"}
            </p>
          </div>
        )}
      </section>
      </> : null}

      {rightWorkspace === "week" ? (
        <Suspense fallback={null}>
          <CalendarWeekView
            anchor={calendarWeekAnchor ?? startOfLocalDay(new Date())}
            initialEvent={calendarEventToOpen}
            onAnchorChange={setCalendarWeekAnchor}
            accounts={calendar.accounts}
            calendars={calendar.calendars}
            onToggleCalendar={(accountId, calendarId, selected) => {
              const next = calendar.calendars
                .filter((option) => option.accountId === accountId && option.selected && option.id !== calendarId)
                .map((option) => option.id);
              if (selected) next.push(calendarId);
              void calendar.setSelection(accountId, next).catch((reason: unknown) => {
                setNotice({ message: errorMessage(reason) });
              });
            }}
            onAddCalendarAccount={() => {
              setRightWorkspace(null);
              openSettingsAt("calendarAccounts");
            }}
            onOpenSettings={() => {
              setRightWorkspace(null);
              openSettingsAt("calendarAccounts");
            }}
            onCreated={() => void refreshCalendarOptions().catch(logBackgroundFailure("Calendar listing"))}
          />
        </Suspense>
      ) : null}
      {rightWorkspace === "calendar" ? (
        <CalendarSidebar
          key={calendarSidebarStart?.key ?? 0}
          initialDate={calendarSidebarStart?.date}
          initialDurationMinutes={calendarSidebarStart?.durationMinutes}
          onClose={() => setRightWorkspace(null)}
          selectedCalendarAccountIds={[...new Set(calendar.calendars.filter((option) => option.selected).map((option) => option.accountId))]}
          availabilityPreferences={availabilityPreferences}
          onDraftAvailability={draftAvailabilityReply}
          draftLabel={correspondence.activeDraft ? "Insert Selected Times" : undefined}
          onOpenSettings={() => {
            setRightWorkspace(null);
            openSettingsAt("calendarAccounts");
          }}
        />
      ) : null}
      {rightWorkspace !== "tasks" && rightWorkspace !== "week" && rightWorkspace !== "contacts" && composingApart && correspondence.liveDraft ? (
        <ComposeContext
          key={correspondence.liveDraft.id}
          draft={correspondence.liveDraft}
          review={correspondence.liveDraft.mode !== "forward" ? (
            <DraftReviewSection key={correspondence.liveDraft.id} actions={correspondence.draftReview}
              available={aiDraftAvailable} onOpenSettings={() => openSettingsAt("ai")} />
          ) : null}
          accounts={accounts}
          onKeyDown={contextPanelKeyDown}
          calendarConnected={calendarConnected}
          preferences={availabilityPreferences}
          taskRefreshKey={taskRevision}
          onAttach={correspondence.context.attachFiles}
          onReplaceRecipient={correspondence.replaceDraftRecipient}
          onSwitchAccount={correspondence.switchDraftAccount}
          onMoveToBcc={correspondence.moveDraftRecipientsToBcc}
          onInsertTimes={draftAvailabilityReply}
          onAddToCalendar={addComposeMeeting}
          onMoreTimes={openCalendarAt}
          onOpenCalendarSettings={() => openSettingsAt("calendarAccounts")}
          onOpenEvent={(event) => openCalendarAt(eventDate(event), availabilityPreferences.defaultDurationMinutes)}
          onOpenThread={openThreadFromDraft}
          onShowMessage={(threadId) => openThreadFromDraft(threadId)}
          onEditTask={(task) => setTaskEditor({ kind: "edit", task })}
          onDraftFollowUp={(task) => void draftFollowUp(task)}
          onTasksChanged={() => { setTaskRevision((current) => current + 1); void refreshTaskIndicators(); }}
        />
      ) : null}
      {rightWorkspace !== "tasks" && rightWorkspace !== "week" && rightWorkspace !== "contacts" && !composingApart ? (
        <ContextPanel
          detail={visibleDetail}
          accounts={accounts}
          selectedEmail={contextPersonEmail}
          reply={composerBelongsToVisibleThread && correspondence.liveDraft ? {
            review: <DraftReviewSection key={correspondence.liveDraft.id} actions={correspondence.draftReview}
              available={aiDraftAvailable} onOpenSettings={() => openSettingsAt("ai")} />,
            recipients: replyRecipients,
            checks: (
              <ReplyChecks
                draft={correspondence.liveDraft}
                accounts={accounts}
                onAttach={correspondence.context.attachFiles}
                onReplaceRecipient={correspondence.replaceDraftRecipient}
                onSwitchAccount={correspondence.switchDraftAccount}
          onMoveToBcc={correspondence.moveDraftRecipientsToBcc}
              />
            ),
            availability: calendarConnected ? (
              <AvailabilitySection
                preferences={availabilityPreferences}
                onInsertTimes={draftAvailabilityReply}
                onAddToCalendar={(slot) => addComposeMeeting(slot, replyRecipients.map((item) => item.email))}
                onMoreTimes={openCalendarAt}
                onOpenCalendarSettings={() => openSettingsAt("calendarAccounts")}
              />
            ) : null,
          } : null}
          onKeyDown={contextPanelKeyDown}
          onOpenThread={jumpToThread}
          onShowMessage={showMessage}
          assist={visibleDetail ? (<>
            <ThreadAssist
              detail={visibleDetail}
              summary={{ enabled: aiSummaryFeatureEnabled, available: aiSummaryAvailable, pending: summaryPending }}
              suggestions={{
                enabled: aiActionFeatureEnabled,
                available: aiActionAvailable,
                requested: actionAnalysisRequested,
                proposals: actionProposals,
                hiddenCount: actionHiddenCount,
                onDiscard: discardActionProposal,
                onReview: reviewActionProposal,
              }}
              scheduling={{
                ...meetingScheduling,
                onAddToCalendar: (_index, proposal, slot) => addMeetingToCalendar(
                  slot,
                  { title: proposal.title, participants: proposal.participants, excerpt: proposal.evidence.excerpt },
                  actionProposalSource ? { from: actionProposalSource, proposal } : null,
                ),
              }}
              loading={actionAnalysisLoading}
              error={summaryError ?? actionAnalysisError}
              preview={actionAnalysisRequested ? actionAnalysisPreview : null}
              onRun={(force) => void runBrief({ force })}
              onOpenSettings={() => openSettingsAt("ai")}
            />
          </>) : null}
          chat={(person) => visibleDetail ? (
            <ThreadChat
              key={visibleDetail.thread.id}
              enabled={aiChatFeatureEnabled}
              available={aiChatAvailable}
              entries={chatByThread[visibleDetail.thread.id] ?? []}
              pending={chatPendingThreads.has(visibleDetail.thread.id)}
              error={chatFailures[visibleDetail.thread.id]?.message ?? null}
              focusRequest={chatFocusRequest}
              attachments={chatAttachmentOptions(visibleDetail.messages)}
              sharedAttachments={sharedChatAttachments(chatByThread[visibleDetail.thread.id] ?? [])}
              onAsk={(question, searchMailbox, attachments) => void askThread(question, searchMailbox, person?.contactId ?? null, attachments)}
              onRetry={() => {
                const failure = chatFailures[visibleDetail.thread.id];
                if (failure) void askThread(failure.question, failure.searchMailbox, person?.contactId ?? null, failure.attachments);
              }}
              onUseReply={(text) => correspondence.replyWithText(text, visibleDetail.messages.at(-1)?.id)}
              onOpenThread={jumpToThread}
              onShowSuggestions={() => document.getElementById(THREAD_ASSIST_ID)?.scrollIntoView?.({ block: "nearest" })}
              onOpenCalendarEvent={(event) => openCalendarView(eventDate(event), event)}
              onOpenSettings={() => openSettingsAt("ai")}
              renderAvailability={(availability) => (
                <MeetingScheduler
                  key={`${availability.rangeStart}|${availability.rangeEnd}|${availability.durationMinutes}`}
                  plan={planChatAvailability(availability, new Date(), availabilityPreferences.defaultDurationMinutes)}
                  {...meetingScheduling}
                  onAddToCalendar={(slot) => addMeetingToCalendar(slot, { title: visibleDetail.thread.subject, participants: [], excerpt: null }, null)}
                />
              )}
            />
          ) : null}
          related={(person, meetingPeople) => visibleDetail ? <>
            <ThreadTasks
              thread={visibleDetail.thread}
              contactId={person?.contactId ?? null}
              refreshKey={taskRevision}
              onAddTask={newTask}
              onEditTask={(task) => setTaskEditor({ kind: "edit", task })}
              onDraftFollowUp={(task) => void draftFollowUp(task)}
              onTasksChanged={() => { setTaskRevision((current) => current + 1); void refreshTaskIndicators(); }}
            />
            {calendarConnected && meetingPeople.length > 0 ? (
              <ContactMeetings
                people={meetingPeople}
                timeZone={availabilityPreferences.timeZone}
                onOpenEvent={(event) => openCalendarView(eventDate(event), event)}
              />
            ) : null}
          </> : null}
        />
      ) : null}
      {rightWorkspace === "contacts" ? <Suspense fallback={null}><ContactsWorkspace key={`${activeAccountId ?? "all"}:${contactAddressBookTarget ?? ""}`} accountId={activeAccountId} onOpenThread={jumpToThread} onSaved={() => { setNotice({ message: "Contact saved" }); void refreshKeepInTouchCount(); }} initialContactId={contactAddressBookTarget} view={contactsView} onViewChange={setContactsView} onKeepInTouchChanged={() => void refreshKeepInTouchCount()} /></Suspense> : null}
      {rightWorkspace === "tasks" ? (
        <TaskSidebar
          ref={taskWorkspaceRef}
          accountId={activeAccountId}
          accountOptions={accounts.map((account) => account.email)}
          onOpenThread={jumpToThread}
          onTasksChanged={() => void refreshTaskIndicators()}
          onDraftFollowUp={(task) => void draftFollowUp(task)}
          refreshKey={taskRevision}
          onCreateTask={createWorkspaceTask}
          onLayoutChange={setTaskLayout}
          onSelectedTaskChange={(task) => {
            setSelectedTaskStatus(task?.status ?? null);
            setSelectedTaskHasThread(Boolean(task?.threadId));
          }}
        />
      ) : null}

      {correspondence.overlay}
      {meetingEventDraft ? (
        <CreateCalendarEventDialog
          start={meetingEventDraft.start}
          end={meetingEventDraft.end}
          accounts={calendar.accounts}
          calendars={calendar.calendars}
          initialTitle={meetingEventDraft.title}
          initialInvitees={meetingEventDraft.invitees}
          initialDescription={meetingEventDraft.description}
          onClose={() => setMeetingEventDraft(null)}
          onCreated={(event) => {
            if (meetingEventDraft.fromOpenedFile) dismissOpenedCalendarFile();
            meetingCreated(event);
          }}
        />
      ) : null}
      {openedCalendarFiles[0] && !meetingEventDraft ? (
        <OpenedCalendarDialog
          key={openedCalendarFiles[0].id}
          file={openedCalendarFiles[0].file}
          waiting={openedCalendarFiles.length - 1}
          calendarConnected={calendarConnected}
          onAddToCalendar={addOpenedInvitationToCalendar}
          onClose={dismissOpenedCalendarFile}
        />
      ) : null}
      {taskEditor ? (
        <TaskEditorDialog
          goals={taskEditorGoals?.accountId === taskEditorAccountId ? taskEditorGoals.goals : null}
          accountId={taskEditorAccountId ?? undefined}
          goalSuggested={taskEditor.kind === "proposal" && Boolean(taskEditor.proposal.goalId)}
          initial={taskEditor.kind === "proposal" ? taskEditor.proposal : taskEditor.kind === "edit" ? taskEditor.task : {
            title: taskEditor.thread.thread.subject,
            kind: "action",
            dueKind: "none",
            timeZone: availabilityPreferences.timeZone,
          }}
          sourceSubject={taskEditor.kind === "edit" ? taskEditor.task.subjectSnapshot : taskEditor.thread.thread.subject}
          evidence={taskEditor.kind === "proposal" ? taskEditor.proposal.evidence.excerpt : taskEditor.kind === "edit" ? taskEditor.task.evidenceText : taskEditor.thread.messages.at(-1)?.bodyText.slice(0, 1000)}
          submitLabel={taskEditor.kind === "proposal" && taskEditor.intent === "edit" ? "Save Proposal" : taskEditor.kind === "edit" ? "Save Task" : "Add Task"}
          onClose={() => setTaskEditor(null)}
          onSubmit={submitTaskEditor}
        />
      ) : null}
      {meetingEditor ? (
        <MeetingProposalDialog
          proposal={meetingEditor.proposal}
          onClose={() => setMeetingEditor(null)}
          onSave={(proposal) => {
            updateActionProposal(meetingEditor.index, proposal);
            setMeetingEditor(null);
          }}
        />
      ) : null}
      {unsubscribeMessage ? (
        <UnsubscribeConfirm
          message={unsubscribeMessage}
          onClose={() => setUnsubscribeMessageId(null)}
          onConfirm={confirmUnsubscribe}
        />
      ) : null}
      {paletteOpen ? (
        <CommandPalette
          context={context}
          execute={executeCommand}
          extraCommands={paletteExtraCommands}
          onClose={() => setPaletteOpen(false)}
        />
      ) : null}
      {shortcutHelpOpen ? (
        <ShortcutHelp extraCommands={paletteExtraCommands} onClose={() => setShortcutHelpOpen(false)} />
      ) : null}
      {labelTargetIds && labelTargetIds.length > 0 ? (
        <LabelManager
          labels={labelTargetAccountId ? labelsByAccount[labelTargetAccountId] ?? [] : []}
          accountId={labelTargetAccountId}
          checkedLabelIds={new Set(
            (labelTargetAccountId ? labelsByAccount[labelTargetAccountId] ?? [] : [])
              .filter((label) =>
                labelTargetIds.every((id) => threads.find((thread) => thread.id === id)?.labels.includes(label.id)),
              )
              .map((label) => label.id),
          )}
          onClose={() => setLabelTargetIds(null)}
          onCreate={createLabel}
          onDelete={deleteLabel}
          onRename={renameLabel}
          onToggle={(label, value) => {
            executeCommand(labelCommand(label.id, label.name, value));
          }}
        />
      ) : null}
      {settingsOpen ? (
        <Suspense fallback={null}>
          <Settings
            section={settingsSection}
            onSectionChange={setSettingsSection}
            onClose={() => setSettingsOpen(false)}
            preferences={preferences}
            mailAccounts={mailAccountSettings}
            calendarAccounts={calendar}
            splitInboxes={splitInboxCatalog}
            labelsByAccount={labelsByAccount}
            snippets={snippetLibrary}
            syncStatus={syncStatus}
            recoveryStatus={recoveryStatus}
            syncDiagnostics={syncDiagnostics}
            onAiConfigChange={refreshAiAvailability}
            onSettingsImported={applyImportedSettings}
          />
        </Suspense>
      ) : null}
      <EnrollmentRequestNotice
        suppressed={settingsOpen && settingsSection === "replicatedSync"}
        onReview={() => openSettingsAt("replicatedSync")}
      />
      <UpdateNotice />
      {notice ? (
        <div className="toast" role="status">
          {notice.message}
          {notice.undo ? <button className="btn-link" onClick={notice.undo}>Undo</button> : null}
          <button className="btn-icon btn-icon-sm" aria-label="Dismiss" onClick={() => setNotice(null)}><X size={ICON_SIZE.sm} /></button>
        </div>
      ) : null}
      {isTabbedMailbox && searchOpen && query.trim() && includeArchived && remoteSearchState === "searching" ? (
        <div className="toast search-status-toast" role="status" aria-live="polite">
          <RefreshCw size={ICON_SIZE.sm} className="spin" />
          Searching Gmail…
        </div>
      ) : null}
      {isTabbedMailbox && searchOpen && query.trim() && includeArchived && remoteSearchState === "error" ? (
        <div className="toast search-status-toast error" role="status" aria-live="polite">
          <CircleAlert size={ICON_SIZE.sm} />
          Gmail search unavailable
        </div>
      ) : null}
      {lightboxImageSrc ? (
        <ImageLightbox src={lightboxImageSrc} onClose={() => setLightboxImageSrc(null)} />
      ) : null}
    </main>
  );
}
