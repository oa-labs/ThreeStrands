import { useSignatures } from "./useSignatures";
import type { FilesWorkspaceSnapshot } from "./FilesWorkspace";
import type { CalendarWorkspaceSnapshot } from "./CalendarWeekView";
import {
  Archive,
  ArrowLeft,
  CalendarDays,
  CircleAlert,
  Command as CommandIcon,
  ContactRound,
  Files,
  Inbox,
  LayoutDashboard,
  ListChecks,
  ListPlus,
  Mail,
  MailCheck,
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
import { GettingStartedDialog, useGettingStarted } from "./GettingStarted";
import { inboxCheckedLabel } from "./inboxZero";
import { InboxZeroSections } from "./InboxZeroSections";
import { SERVER_SEARCH_FAILED_COPY, SERVER_SEARCHING_COPY } from "./searchStatusCopy";
import type { GettingStartedStepId } from "./gettingStartedSteps";
import { CalendarSidebar } from "./CalendarSidebar";
import { eventDate, startOfLocalDay } from "./calendarTime";
import { chatAttachmentOptions } from "./chatAttachments";
import { labelCommand, undoResult, type CommandResult, type MailboxKind } from "./commands";
import { FILES_CHANGED } from "./FileStarButton";
import { draftRecipients } from "./composeChecks";
import { ComposeContext, ReplyChecks } from "./ComposeContext";
import { ContactCardContext } from "./ContactCard";
import { ContactMeetings } from "./ContactMeetings";
import { ContextPanel } from "./ContextPanel";
import { focusContextPanel, handleContextPanelKeyDown } from "./contextPanelFocus";
import { CreateCalendarEventDialog } from "./CreateCalendarEventDialog";
import { mailClient } from "./data/client";
import type { ActionProposal, SenderRule } from "./domain";
import { EnrollmentRequestNotice } from "./EnrollmentRequestNotice";
import { AvailableAccountsNotice } from "./AvailableAccountsNotice";
import { errorMessage, logBackgroundFailure } from "./errors";
import { FolderSwitcher } from "./FolderSwitcher";
import { ICON_SIZE } from "./iconSizes";
import { ImageLightbox } from "./ImageLightbox";
import { LabelManager } from "./LabelManager";
import { conversationLabelGroups, formatLabelName, labelCapabilities } from "./labels";
import { MeetingProposalDialog } from "./MeetingProposalDialog";
import { FindTimesCard } from "./FindTimesCard";
import { MessageCard } from "./MessageCard";
import { OpenedCalendarDialog } from "./OpenedCalendarDialog";
import { PanelResizeHandle, useInboxWidth } from "./PanelResizeHandle";
import { messagesWithQueuedReplies, queuedMessage } from "./queuedReplies";
import { AvailabilitySection } from "./RecipientSections";
import { planChatAvailability, suggestedSearch } from "./scheduling";
import { SearchField } from "./SearchField";
import { readSelectedAccountId, readSelectedMailboxForAccount } from "./settings";
import type { SettingsSection } from "./settingsPanelTypes";
import { readSettingsSection, saveSettingsSection } from "./settingsNavigation";
import { TaskEditorDialog } from "./TaskEditorDialog";
import { TaskSidebar, type TaskWorkspaceSnapshot } from "./TaskSidebar";
import { THREAD_ASSIST_ID, ThreadAssist } from "./ThreadAssist";
import { latestInvitationMessage } from "./calendarInvitations";
import { sharedChatAttachments, ThreadChat } from "./ThreadChat";
import { ThreadList, ThreadRow } from "./ThreadList";
import { ThreadTasks } from "./ThreadTasks";
import { threadTextIndex } from "./threadTextIndex";
import { UnsubscribeConfirm } from "./UnsubscribeConfirm";
import { SenderSplitDialog } from "./SenderSplitDialog";
import { MuteSenderDialog } from "./MuteSenderDialog";
import { useSenderRules } from "./useSenderRules";
import { describeSenderRule } from "./MutedSendersSettings";
import type { SenderActions, SenderTarget } from "./SenderSections";
import { UpdateNotice } from "./UpdateNotice";
import { useAccounts } from "./useAccounts";
import { useAiAvailability } from "./useAiAvailability";
import { useAppCommandContext } from "./useAppCommandContext";
import { useAppPreferences } from "./useAppPreferences";
import { useCommandExecution, useCommandUndo } from "./useCommandExecution";
import { ThreadDrafts, threadDrafts } from "./ThreadDrafts";
import { OutboxDetail, OutboxList, useCorrespondence } from "./useCorrespondence";
import { DraftsList, useDraftSelection } from "./DraftsList";
import { useCheckedSelection } from "./useCheckedSelection";
import { checkableOutboxKeys, outboxBatchAbility } from "./outboxView";
import { useOutboxSelection } from "./useOutboxSelection";
import { useLabelCatalog } from "./useLabelCatalog";
import { loadSearchContacts } from "./searchContacts";
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
import { useSentReplyRefresh } from "./useSentReplyRefresh";
import { useThreadIntelligence } from "./useThreadIntelligence";
import { useThreadMutations } from "./useThreadMutations";
import { useThreadSelection } from "./useThreadSelection";
import { useTriageSession } from "./useTriageSession";
import { useWorkspaces } from "./useWorkspaces";
import { ErrorBoundary } from "./ErrorBoundary";
export { AccountSwitcher } from "./AccountSwitcher";
export { messagesWithQueuedReplies } from "./queuedReplies";
export { NOTICE_TIMEOUT_MS } from "./useNotice";
export { formatMailTimestamp } from "./threadPresentation";

// Rarely opened workspaces load on demand so they stay off the startup bundle.
const Settings = lazy(() => import("./SettingsPanel").then((m) => ({ default: m.Settings })));
const CalendarWeekView = lazy(() => import("./CalendarWeekView").then((m) => ({ default: m.CalendarWeekView })));
const TodayWorkspace = lazy(() => import("./TodayWorkspace").then((m) => ({ default: m.TodayWorkspace })));
const FilesWorkspace = lazy(() => import("./FilesWorkspace").then((m) => ({ default: m.FilesWorkspace })));
const ContactsWorkspace = lazy(() => import("./ContactsWorkspace").then((m) => ({ default: m.ContactsWorkspace })));

// An Outbox message is shown alone and expanded: it has no conversation to
// activate, collapse, scroll through, or reply within.
const ignoreOutboxMessageEvent = () => {};

export function App() {
  const inboxSize = useInboxWidth();
  const preferences = useAppPreferences();
  const {
    effectiveTheme: effectiveThemeValue,
    toggleTheme,
    fontScale,
    fontFamily,
    adjustFontScale,
    emailZoom,
    emailMinimumFontSize,
    emailTextColors,
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
    newlyAvailableAccounts,
    acknowledgeAvailableAccounts,
  } = mailAccounts;
  const viewingAccountSettings = settingsOpen && settingsSection === "accounts";
  // Seeing the account list counts as seeing the accounts offered there.
  useEffect(() => {
    if (viewingAccountSettings && newlyAvailableAccounts.length > 0) acknowledgeAvailableAccounts();
  }, [viewingAccountSettings, newlyAvailableAccounts, acknowledgeAvailableAccounts]);
  const { unreadCounts, checkingMail, refreshUnreadCounts } = useUnreadCounts(syncStatus, setSyncStatus);
  const [activeSplitInboxId, setActiveSplitInboxId] = useState<string | null>(null);
  const [mailbox, setMailbox] = useState<MailboxKind>(() => readSelectedMailboxForAccount(readSelectedAccountId()) ?? "inbox");
  const mailboxThreads = useMailboxThreads({
    activeAccountId, mailbox, activeSplitInboxId, setSelectedId, refreshUnreadCounts,
    refreshMailboxUnreadCounts,
  });
  const {
    threads, setThreads, query, setQuery, queryRef, searchOpen, setSearchOpen, includeArchived,
    inboxOnly, setInboxOnly, remoteSearchState, searchStatus, setSearchStatus, hasMoreResults, loading,
    mailboxError, loadingMoreState,
    loadThreads, loadThreadsRef, loadMoreResults, contextOpenedThreadRef,
  } = mailboxThreads;
  const selection = useThreadSelection({ threads, selectedId, setSelectedId, query, includeArchived, contextOpenedThreadRef });
  const {
    checkedIds, setCheckedIds, selectedThreadRowRef, selectAllRef, activeMessageFilters,
    toggleMessageFilter, clearMessageFilters, visibleThreads, selected, selectThread, applyThreadSelectionGesture,
    toggleChecked,
  } = selection;
  const [notice, setNotice] = useNotice();
  const threadDetail = useThreadDetail(selectedId, threads, setNotice);
  const { detail, setDetail, visibleDetail, detailLoading } = threadDetail;
  const { applyThreadSummary } = useMailStateActions({ mailboxThreads, threadDetail });
  const snippetLibrary = useSnippets();
  const signatureLibrary = useSignatures();
  const correspondence = useCorrespondence(
    accounts, visibleDetail?.messages.at(-1)?.id, visibleDetail?.thread.accountId,
    snippetLibrary.snippets, snippetLibrary.create, snippetLibrary.update, snippetLibrary.remove,
    selectedId,
    { theme: effectiveThemeValue, fontScale: fontScale / 100, fontFamily, emailZoom, emailMinimumFontSize, emailTextColors, loadImages: loadRemoteImages },
    signatureLibrary,
  );
  const draftSelection = useDraftSelection(correspondence.drafts, correspondence.activeDraft?.id ?? null);
  const composerBelongsToVisibleThread = Boolean(
    correspondence.activeDraft
    && correspondence.activeDraft.mode !== "new"
    && (visibleDetail?.messages.some((message) => message.id === correspondence.activeDraft?.sourceId)
      // A reply written in another app knows its conversation, not its message.
      || (visibleDetail && !correspondence.activeDraft.sourceId && threadDrafts([correspondence.activeDraft], visibleDetail.thread).length > 0)),
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
  useSentReplyRefresh(visibleDetail, displayedMessages, correspondence.outbox, setDetail);
  const displayedThreadText = useMemo(() => threadTextIndex(displayedMessages), [displayedMessages]);
  const outboxSelection = useOutboxSelection(correspondence.outbox, correspondence.summaries);
  const checkableOutbox = useMemo(() => checkableOutboxKeys(outboxSelection.entries), [outboxSelection.entries]);
  const outboxChecks = useCheckedSelection(checkableOutbox, outboxSelection.selectedEntry?.key ?? null);
  const checkedOutboxEntries = outboxSelection.entries.filter((entry) => outboxChecks.checkedIds.has(entry.key));
  const returnableSends = checkedOutboxEntries.filter((entry) => outboxBatchAbility(entry).returnable).flatMap((entry) => entry.kind === "local" ? [entry.item.id] : []);
  const discardableSends = checkedOutboxEntries.filter((entry) => outboxBatchAbility(entry).discardable).flatMap((entry) => entry.kind === "local" ? [entry.item.id] : []);
  const selectedOutboxItem = outboxSelection.selectedEntry?.kind === "local" ? outboxSelection.selectedEntry.item : undefined;
  const selectedOutboxMessage = useMemo(
    () => selectedOutboxItem ? queuedMessage(selectedOutboxItem, `outbox-${selectedOutboxItem.id}`) : null,
    [selectedOutboxItem],
  );
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
    readingMessageId,
    readingMessageIdRef,
    latestDisplayedMessageId,
  } = reader;
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [shortcutHelpOpen, setShortcutHelpOpen] = useState(false);
  const [gettingStartedOpen, setGettingStartedOpen] = useState(false);
  const [unsubscribeMessageId, setUnsubscribeMessageId] = useState<string | null>(null);
  const [splitTarget, setSplitTarget] = useState<SenderTarget | null>(null);
  const [muteTarget, setMuteTarget] = useState<{ target: SenderTarget; inboxThreadIds: string[] } | null>(null);
  const fileSessions = useRef(new Map<string, FilesWorkspaceSnapshot>());
  const calendarSession = useRef<CalendarWorkspaceSnapshot | null>(null);
  const taskSessions = useRef(new Map<string, TaskWorkspaceSnapshot>());
  const workspaces = useWorkspaces({ settingsOpen, settingsSection, visibleDetail, readingMessageId, readingMessageIdRef, openSettingsAt, setNotice });
  const {
    rightWorkspace, setRightWorkspace, calendarWeekAnchor, setCalendarWeekAnchor,
    calendarEventToOpen, openCalendarAt, contactAddressBookTarget, contactsView, setContactsView,
    keepInTouchDueCount, refreshKeepInTouchCount, calendar, refreshCalendarOptions,
    calendarConnected, calendarSidebarStart, openContactsView, contactCardActions,
    scheduleShown, setSideColumn, showMailContext,
    contextPersonEmail, openCalendarView, openTasksView, openKeepInTouchView,
  } = workspaces;
  const { openTaskThreadIds, taskRevision, setTaskRevision, refreshTaskIndicators } = useTaskIndicators(activeAccountId);
  const splitInboxCatalog = useSplitInboxes();
  const senderRuleCatalog = useSenderRules();
  const { refresh: refreshSplitInboxes } = splitInboxCatalog;
  const isThreadMailbox = mailbox === "inbox" || mailbox === "allMail" || mailbox === "sent" || mailbox === "trash" || mailbox === "split";
  const isTabbedMailbox = mailbox === "inbox" || mailbox === "split";
  const navigation = useMailNavigation({
    mailboxThreads, threadDetail, reader, workspaces, correspondence, splitInboxCatalog, mailbox,
    setMailbox, activeSplitInboxId, setActiveSplitInboxId, activeAccountId, setActiveAccountId,
    selectedId, setSelectedId, selectedThreadRowRef, setNotice,
  });
  const { accountSplitInboxes, mailboxTitle, returnStep, openTaskThread, jumpToThread } = navigation;
  // Only a loaded, unfiltered, unsearched Main inbox with no threads is inbox zero;
  // a failed load or an empty split is not.
  const inboxZero = mailbox === "inbox" && !loading && !mailboxError && threads.length === 0 && !query.trim() && accounts.length > 0;
  const inboxChecked = inboxZero
    ? inboxCheckedLabel(syncStatus, activeAccountId ? accounts.filter((account) => account.email === activeAccountId) : accounts)
    : null;
  const { refreshMail, recoveryStatus, syncDiagnostics } = useMailRefresh({ query, loadThreadsRef, setSyncStatus, refreshTaskIndicators });
  const [labelTargetIds, setLabelTargetIds] = useState<string[] | null>(null);
  // Label ids belong to an account. Bulk actions target one account, so the
  // first target thread determines which catalog the label manager shows.
  const labelTargetAccountId = labelTargetIds?.length
    ? threads.find((thread) => thread.id === labelTargetIds[0])?.accountId ?? activeAccountId ?? undefined
    : undefined;
  const [lightboxImageSrc, setLightboxImageSrc] = useState<string | null>(null);
  const {
    aiProviderChosen, aiProviderLabel, aiSummaryAvailable, aiSummaryFeatureEnabled, aiProactive, aiActionFeatureEnabled,
    aiActionAvailable, aiChatFeatureEnabled, aiChatAvailable, aiDraftFeatureEnabled, aiDraftAvailable, refreshAiAvailability,
  } = useAiAvailability(settingsOpen);
  const {
    mailAccountSettings, applyImportedSettings,
  } = useSettingsIntegration({ mailAccounts, preferences, mailboxThreads, refreshSplitInboxes, refreshAiAvailability, setSettingsSection });
  const {
    labelsByAccount, createLabel, deleteLabel, renameLabel,
  } = useLabelCatalog(accounts, detail?.thread.accountId, labelTargetAccountId, () => loadThreads(query));
  const undo = useCommandUndo(setNotice);
  const gettingStarted = useGettingStarted({
    mailConnected: accounts.length > 0,
    calendarConnected,
    aiReady: aiSummaryAvailable || aiChatAvailable || aiDraftAvailable || aiActionAvailable,
  });
  const completeGettingStartedStep = gettingStarted.complete;
  useEffect(() => { if (paletteOpen) completeGettingStartedStep("commandPalette"); }, [paletteOpen, completeGettingStartedStep]);
  useEffect(() => { if (shortcutHelpOpen) completeGettingStartedStep("shortcuts"); }, [shortcutHelpOpen, completeGettingStartedStep]);
  useEffect(() => { if (undo.canUndoArchive) completeGettingStartedStep("archive"); }, [undo.canUndoArchive, completeGettingStartedStep]);
  const { rememberUndo } = undo;
  /** Discards like the inbox trashes: at once, with Undo in the notice and on z. */
  const discardWithUndo = useCallback(async (discard: () => Promise<string[]>, describe: (count: number) => string) => {
    const discarded = await discard();
    if (discarded.length === 0) return;
    const result: CommandResult = {
      message: describe(discarded.length),
      undoAction: () => correspondence.restoreDiscardedDrafts(discarded),
    };
    rememberUndo({ id: "drafts.discard", title: "Discard", keys: [], group: "Compose", enabled: () => true, run: async () => result, undo: undoResult }, result);
  }, [correspondence, rememberUndo]);
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
  const invitationMessage = visibleDetail ? latestInvitationMessage(visibleDetail) : null;
  const canUnsubscribe = Boolean(latestMessage?.unsubscribe?.methods.length);
  const unsubscribeMessage = visibleDetail?.messages.find((message) => message.id === unsubscribeMessageId) ?? null;
  const intelligence = useThreadIntelligence({
    accounts, selected, visibleDetail, isThreadMailbox, autoReadDelaySeconds,
    availabilityPreferences, applyThreadSummary, aiProactive, aiSummaryAvailable,
    aiActionAvailable, aiActionFeatureEnabled,
  });
  const {
    summaryPending, summaryError, runBrief, actionProposals, actionProposalSource,
    actionAnalysisLoading, actionAnalysisError, actionAnalysisRequested,
    actionHiddenCount, actionExtraCount, chatByThread, chatPendingThreads, chatFailures, chatFocusRequest, askThread,
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
    meetingCreated, meetingScheduling, draftAvailabilityReply,
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
    showMailContext();
    focusThreadChat();
  }, [focusThreadChat, showMailContext]);

  /** Shows the context panel's AI section and fetches suggestions if missing. */
  const getSuggestions = useCallback(() => {
    showMailContext();
    window.requestAnimationFrame(() => document.getElementById(THREAD_ASSIST_ID)?.scrollIntoView?.({ block: "nearest" }));
    void runBrief({ only: "suggestions" });
  }, [runBrief, showMailContext]);

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
  // Clicking another conversation in the list does the same; a reply stays open over its own conversation.
  const selectThreadFromList = useCallback((threadId: string) => {
    if (!correspondence.activeDraft || (!composingApart && threadId === selectedId)) {
      selectThread(threadId);
      return;
    }
    void correspondence.flushDraft()
      .then(() => {
        correspondence.context.openInbox();
        selectThread(threadId);
      })
      .catch((reason: unknown) => setNotice({ message: errorMessage(reason) }));
  }, [composingApart, correspondence, selectThread, selectedId, setNotice]);
  const {
    selectAdjacentMessage, activateMessage, toggleMessage, showMessage, showMessageLink, linkReveal, registerMessageNode,
    respondToMessage,
    editMessageAsNew,
  } = useReaderActions({ reader, displayedMessages, visibleDetail, selected, correspondence, jumpToThread, mailbox, includeArchived, recordTriageEvent });

  const interactionScope = correspondence.activeDraft
    ? "compose"
    : paletteOpen
      ? "palette"
      : settingsOpen || shortcutHelpOpen || gettingStartedOpen || Boolean(unsubscribeMessage) || Boolean(splitTarget) || Boolean(muteTarget) || Boolean(labelTargetIds?.length) || Boolean(taskEditor) || Boolean(meetingEditor)
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

  const closeSearch = useCallback(() => {
    setQuery("");
    setSearchOpen(false);
    selectedThreadRowRef.current?.focus();
  }, [selectedThreadRowRef, setQuery, setSearchOpen]);
  const context = useAppCommandContext({
    correspondence, reader, workspaces, navigation, selection, tasks, undo, mailboxThreads,
    runBrief, recordTriageEvent, displayedMessages, composerBelongsToVisibleThread, outboxSelection, draftSelection, selectAllOutbox: outboxChecks.selectAll, mutateIds,
    view: { mailbox, setMailbox, setActiveSplitInboxId, activeAccountId, selectedId, setSelectedId, isTabbedMailbox },
    ui: { interactionScope, canUnsubscribe, latestMessage, aiSummaryAvailable,
      aiBriefEnabled: aiSummaryFeatureEnabled || aiActionFeatureEnabled, aiChatEnabled: aiChatFeatureEnabled, aiDraftEnabled: aiDraftFeatureEnabled,
      labelTargetIds, setLabelTargetIds, setUnsubscribeMessageId, setPaletteOpen, setShortcutHelpOpen, setGettingStartedOpen, openSettingsAt, adjustFontScale, toggleTheme, toggleContextPanelFocus, selectAdjacentMessage, getSuggestions, openThreadChat, refreshMail, closeSearch },
  });

  const {
    executeCommand, executeById, paletteExtraCommands, runOnSelection,
  } = useCommandExecution({ context, accounts, accountSplitInboxes, checkedIds, mutateIds, rememberUndo, setNotice });
  const senderActions = useMemo<SenderActions>(() => ({
    splits: splitInboxCatalog.splitInboxes,
    onUnsubscribe: canUnsubscribe ? () => executeById("thread.unsubscribe") : undefined,
    // Reaches conversations the list has not paged in, and offers Undo like any archive.
    onArchiveThreads: (ids) => executeCommand({
      id: "sender.archiveInbox",
      title: "Archive",
      keys: [],
      group: "Triage",
      enabled: () => true,
      run: () => mutateIds(ids, { kind: "archive", value: true }, { includeUnloaded: true }),
      undo: undoResult,
    }),
    onNewSplit: setSplitTarget,
    rules: senderRuleCatalog.senderRules,
    onMute: (target, inboxThreadIds) => setMuteTarget({ target, inboxThreadIds }),
    onUnmute: (rule) => {
      senderRuleCatalog.unmute(rule.id)
        .then(() => setNotice({ message: `Unmuted ${describeSenderRule(rule)}` }))
        .catch((reason: unknown) => setNotice({ message: `Unmute failed: ${errorMessage(reason)}` }));
    },
  }), [splitInboxCatalog.splitInboxes, senderRuleCatalog, canUnsubscribe, executeById, executeCommand, mutateIds, setNotice]);

  // Files lists nothing from spam, so the list re-reads after the change and after Undo.
  const markFileSourceSpam = useCallback((threadId: string) => executeCommand({
    id: "files.markSpam",
    title: "Mark Spam",
    keys: [],
    group: "Triage",
    enabled: () => true,
    run: async () => {
      const result = await mutateIds([threadId], { kind: "spam", value: true }, { includeUnloaded: true });
      window.dispatchEvent(new Event(FILES_CHANGED));
      const undo = result.undoAction;
      return undo ? { ...result, undoAction: async () => { await undo(); window.dispatchEvent(new Event(FILES_CHANGED)); } } : result;
    },
    undo: undoResult,
  }), [executeCommand, mutateIds]);

  /**
   * Mutes a sender, archiving the chosen Inbox conversations in the same
   * step. Undo removes the rule and puts those conversations back as they
   * were, unread ones unread.
   */
  const muteSender = useCallback(async (target: SenderTarget, matchKind: SenderRule["matchKind"], matchValue: string, archiveThreadIds: string[]) => {
    const result = await senderRuleCatalog.mute(target.accountId, matchKind, matchValue, archiveThreadIds);
    setMuteTarget(null);
    if (result.archivedThreadIds.length > 0) void loadThreadsRef.current(queryRef.current);
    const archived = result.archivedThreadIds.length;
    setNotice({
      message: `Muted ${describeSenderRule(result.rule)}${archived === 0 ? "" : archived === 1 ? " and archived 1 conversation" : ` and archived ${archived} conversations`}`,
      undo: () => {
        setNotice(null);
        void (async () => {
          await senderRuleCatalog.unmute(result.rule.id);
          await mailClient.mutateThreads([
            ...result.archivedThreadIds.map((threadId) => ({ kind: "archive" as const, threadId, value: false })),
            ...result.unreadThreadIds.map((threadId) => ({ kind: "read" as const, threadId, value: false })),
          ]);
          await loadThreadsRef.current(queryRef.current);
        })().catch((reason: unknown) => setNotice({ message: `Undo failed: ${errorMessage(reason)}` }));
      },
    });
  }, [senderRuleCatalog, loadThreadsRef, queryRef, setNotice]);
  const runGettingStartedStep = useCallback((id: GettingStartedStepId) => {
    setGettingStartedOpen(false);
    if (id === "connectMail") openSettingsAt("accounts");
    else if (id === "connectCalendar") openSettingsAt("calendarAccounts");
    else if (id === "setUpAi") openSettingsAt("ai");
    else if (id === "commandPalette") setPaletteOpen(true);
    else if (id === "shortcuts") setShortcutHelpOpen(true);
    else executeById("view.mail");
  }, [executeById, openSettingsAt]);

  const reorderNavbarAccounts = useCallback((emails: string[]) => {
    void reorderAccounts(emails).catch(() => {
      setNotice({ message: "Account order could not be saved" });
    });
  }, [reorderAccounts, setNotice]);

  const clearContextOpenedThread = useCallback(() => {
    contextOpenedThreadRef.current = null;
  }, [contextOpenedThreadRef]);
  const toggleInboxOnly = useCallback(() => setInboxOnly((current) => !current), [setInboxOnly]);
  // Search suggestions draw on the account(s) being searched: the active one,
  // or every account in the merged view.
  const searchAccountIds = useMemo(
    () => (activeAccountId ? [activeAccountId] : accounts.map((account) => account.email)),
    [activeAccountId, accounts],
  );
  const searchLabelNames = useMemo(
    () => searchAccountIds.flatMap((accountId) =>
      (labelsByAccount[accountId] ?? []).filter((label) => label.kind === "user").map((label) => label.name)),
    [searchAccountIds, labelsByAccount],
  );
  const loadSearchSuggestionContacts = useCallback(
    (contactQuery: string) => loadSearchContacts(searchAccountIds, contactQuery),
    [searchAccountIds],
  );

  const showNoticeMessage = useCallback((message: string) => setNotice({ message }), [setNotice]);

  const mailInView = rightWorkspace === null;
  const selectedThreads = threads.filter((thread) => checkedIds.has(thread.id));
  const allSelectedThreadsStarred = selectedThreads.length > 0
    && selectedThreads.every((thread) => thread.starred);
  const batchStarLabel = allSelectedThreadsStarred ? "Unstar" : "Star";

  return (
    <main className={`app-shell${rightWorkspace === "today" ? " today-open" : rightWorkspace === "files" ? " files-open" : rightWorkspace === "tasks" ? " tasks-open" : rightWorkspace === "contacts" ? " contacts-open" : rightWorkspace === "week" ? " week-open" : ""}`} style={{ "--inbox-width": `${inboxSize.width}px` } as CSSProperties}>
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
          <HoverTooltip label="Today" shortcut="0"><button className={`nav-button ${rightWorkspace === "today" ? "active" : ""}`} aria-label="Today (0)" onClick={() => executeById("view.today")}><LayoutDashboard size={ICON_SIZE.lg} /></button></HoverTooltip>
          <HoverTooltip label="Inbox" shortcut="1">
            <button
              className={`nav-button ${mailInView ? "active" : ""}`}
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
          <HoverTooltip label="Files" shortcut="5"><button className={`nav-button ${rightWorkspace === "files" ? "active" : ""}`} aria-label="Files (5)" onClick={() => executeById("view.files")}><Files size={ICON_SIZE.lg} /></button></HoverTooltip>
          <hr className="sidebar-nav-separator" aria-hidden="true" />
          <HoverTooltip title={checkingMail ? "Checking for mail…" : "Refresh mail (⌘/Ctrl+R)"}><button className="nav-button" aria-label="Refresh mail" aria-keyshortcuts="Meta+R Control+R" aria-busy={checkingMail} onClick={() => executeById("mail.refresh")}>
            <RefreshCw size={ICON_SIZE.lg} className={checkingMail ? "spin" : ""} />
          </button></HoverTooltip>
          <HoverTooltip label={`Switch to ${effectiveThemeValue === "dark" ? "light" : "dark"} mode`} shortcut="⌘D">
            <button className="nav-button" aria-label={`Switch to ${effectiveThemeValue === "dark" ? "light" : "dark"} mode`} aria-keyshortcuts="Meta+D Control+D" onClick={() => executeById("theme.toggle")}>
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
          {gettingStarted.visible ? (
            <HoverTooltip title={`Getting started · ${gettingStarted.remaining} left`}>
              <button className="nav-button" aria-label={`Getting Started, ${gettingStarted.remaining} steps left`} onClick={() => setGettingStartedOpen(true)}>
                <ListChecks size={ICON_SIZE.lg} />
                <span className="nav-button-badge" aria-hidden="true">{gettingStarted.remaining}</span>
              </button>
            </HoverTooltip>
          ) : null}
          <HoverTooltip title="Settings (⌘,)"><button className="nav-button" aria-label="Settings (⌘,)" onClick={() => executeById("settings.open")}>
            <SettingsIcon size={ICON_SIZE.lg} />
          </button></HoverTooltip>
        </div>
      </nav>

      {mailInView ? <>
      <section id="inbox-panel" className="thread-column" aria-label="Inbox">
        <PanelResizeHandle {...inboxSize} label="Resize Inbox" controlsId="inbox-panel" title="Drag to resize inbox. Use arrow keys to adjust; double-click to reset." />
        <ErrorBoundary area="the inbox" resetKey={`${activeAccountId ?? "all"}:${mailbox}`}>
        <header className="thread-header">
          {mailbox === "drafts" && draftSelection.checkedIds.size > 0 ? (
            <div className="batch-toolbar" role="toolbar" aria-label="Batch actions">
              <label className="select-all">
                <input
                  type="checkbox"
                  ref={(input) => {
                    if (input) input.indeterminate = draftSelection.checkedIds.size < correspondence.drafts.length;
                  }}
                  checked={draftSelection.checkedIds.size === correspondence.drafts.length}
                  aria-label="Select All Drafts" title="Select all drafts (⌘/Ctrl+A)" aria-keyshortcuts="Meta+A Control+A"
                  onChange={(event) => event.target.checked ? draftSelection.selectAll() : draftSelection.clear()}
                />
              </label>
              <span className="batch-count">{draftSelection.checkedIds.size} selected</span>
              <div className="batch-actions" style={{ "--batch-columns": 2 } as CSSProperties}>
                <HoverTooltip label="Discard" placement="bottom">
                  <ActionButton label="Discard" onClick={() => {
                    const ids = [...draftSelection.checkedIds];
                    draftSelection.clear();
                    void discardWithUndo(() => correspondence.discardListedDrafts(ids), (count) => count === 1 ? "Draft discarded" : `${count} drafts discarded`);
                  }}>
                    <Trash size={ICON_SIZE.md} />
                  </ActionButton>
                </HoverTooltip>
                <HoverTooltip label="Clear selection" placement="bottom">
                  <button
                    className="btn-icon"
                    aria-label="Clear Selection" title="Clear selection (Esc)" aria-keyshortcuts="Escape"
                    onClick={draftSelection.clear}
                  >
                    <X size={ICON_SIZE.lg} />
                  </button>
                </HoverTooltip>
              </div>
            </div>
          ) : mailbox === "outbox" && outboxChecks.checkedIds.size > 0 ? (
            <div className="batch-toolbar" role="toolbar" aria-label="Batch actions">
              <label className="select-all">
                <input
                  type="checkbox"
                  ref={(input) => {
                    if (input) input.indeterminate = outboxChecks.checkedIds.size < checkableOutbox.length;
                  }}
                  checked={outboxChecks.checkedIds.size === checkableOutbox.length}
                  aria-label="Select All Messages" title="Select all messages you can act on (⌘/Ctrl+A)" aria-keyshortcuts="Meta+A Control+A"
                  onChange={(event) => event.target.checked ? outboxChecks.selectAll() : outboxChecks.clear()}
                />
              </label>
              <span className="batch-count">{outboxChecks.checkedIds.size} selected</span>
              <div className="batch-actions" style={{ "--batch-columns": 3 } as CSSProperties}>
                <HoverTooltip label={returnableSends.length ? `Return ${returnableSends.length} to Drafts` : "Nothing selected can return to Drafts"} placement="bottom">
                  <ActionButton
                    label={`Return ${returnableSends.length} to Drafts`}
                    disabled={returnableSends.length === 0}
                    onClick={() => {
                      const ids = returnableSends;
                      outboxChecks.clear();
                      void correspondence.returnSendsToDrafts(ids);
                    }}
                  >
                    <RotateCcw size={ICON_SIZE.md} />
                  </ActionButton>
                </HoverTooltip>
                <HoverTooltip label={discardableSends.length ? `Discard ${discardableSends.length} failed` : "Only failed messages can be discarded"} placement="bottom">
                  <ActionButton
                    label={`Discard ${discardableSends.length} Failed`}
                    disabled={discardableSends.length === 0}
                    onClick={() => {
                      const ids = discardableSends;
                      outboxChecks.clear();
                      void discardWithUndo(() => correspondence.discardFailedSends(ids), (count) => count === 1 ? "Failed message discarded" : `${count} failed messages discarded`);
                    }}
                  >
                    <Trash size={ICON_SIZE.md} />
                  </ActionButton>
                </HoverTooltip>
                <HoverTooltip label="Clear selection" placement="bottom">
                  <button
                    className="btn-icon"
                    aria-label="Clear Selection" title="Clear selection (Esc)" aria-keyshortcuts="Escape"
                    onClick={outboxChecks.clear}
                  >
                    <X size={ICON_SIZE.lg} />
                  </button>
                </HoverTooltip>
              </div>
            </div>
          ) : isThreadMailbox && checkedIds.size > 0 ? (
            <div className="batch-toolbar" role="toolbar" aria-label="Batch actions">
              <label className="select-all">
                <input
                  ref={selectAllRef}
                  type="checkbox"
                  checked={visibleThreads.length > 0 && visibleThreads.every((thread) => checkedIds.has(thread.id))}
                  aria-label="Select All Conversations" title="Select all visible conversations (⌘/Ctrl+A)" aria-keyshortcuts="Meta+A Control+A"
                  onChange={(event) =>
                    event.target.checked ? context.selectAllConversations?.() : setCheckedIds(new Set())
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
                    aria-label="Clear Selection" title="Clear selection (Esc)" aria-keyshortcuts="Escape"
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
                <div className="mailbox-heading">
                  <div className="mailbox-heading-context">
                    <FolderSwitcher
                      selected={isTabbedMailbox ? "inbox" : mailbox}
                      inboxUnreadCount={mailboxUnreadCounts.inbox}
                      draftCount={correspondence.draftCount}
                      outboxCount={correspondence.outboxCount}
                      onSelect={(folder) => {
                        // A pointer choice in the menu is explicit, so it works
                        // while a draft is open; the composer saves on the way out.
                        if (folder === "inbox") context.openInbox();
                        else if (folder === "allMail") context.openAllMail();
                        else if (folder === "sent") context.openSent();
                        else if (folder === "drafts") context.openDrafts();
                        else if (folder === "outbox") context.openOutbox();
                        else context.openTrash();
                      }}
                    />
                    <span className="eyebrow-account">· {activeAccountId ?? "All accounts"}</span>
                  </div>
                  <div className="mailbox-title-row">
                    <h1>
                      {mailbox === "drafts"
                        ? `${correspondence.drafts.length} drafts`
                        : mailbox === "outbox"
                          ? `${outboxSelection.entries.length} outgoing`
                          : `${visibleThreads.length} conversations`}
                    </h1>
                    {isThreadMailbox ? (
                      <div className="mailbox-header-actions">
                        {isTabbedMailbox ? (
                          <HoverTooltip label="Add split inbox" placement="bottom">
                            <button type="button" className="btn-icon" aria-label="Add Split Inbox" onClick={() => openSettingsAt("splitInboxes")}>
                              <ListPlus size={ICON_SIZE.md} />
                            </button>
                          </HoverTooltip>
                        ) : null}
                        <FiltersButton activeFilters={activeMessageFilters} onToggleFilter={toggleMessageFilter} />
                      </div>
                    ) : null}
                  </div>
                  {/* One tab is not a choice, so Main appears only beside at least one split. */}
                  {isTabbedMailbox && accountSplitInboxes.length > 0 ? (
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
                    </div>
                  ) : null}
                </div>
              </div>
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
              inboxOnly={inboxOnly}
              onToggleInboxOnly={toggleInboxOnly}
              labels={searchLabelNames}
              loadContacts={loadSearchSuggestionContacts}
            />
          </div>
        ) : null}
        <ThreadList role={isThreadMailbox || mailbox === "outbox" || mailbox === "drafts" ? "listbox" : "list"} aria-label={mailboxTitle}>
          {mailbox === "drafts" ? (<>
            <DraftsList
              drafts={correspondence.drafts}
              openId={correspondence.activeDraft?.id ?? null}
              checkedIds={draftSelection.checkedIds}
              showAccount={accounts.length > 1}
              accountColors={accountColors}
              onOpen={(draft) => { draftSelection.open(draft.id); correspondence.openDraft(draft); }}
              onDiscard={(draft) => void discardWithUndo(() => correspondence.discardListedDraft(draft), () => "Draft discarded")}
              onToggleCheck={draftSelection.toggle}
              onSelectionGesture={draftSelection.applyGesture}
            />
          </>
          ) : mailbox === "outbox" ? (<>
            <OutboxList
              entries={outboxSelection.entries}
              selectedKey={outboxSelection.selectedEntry?.key ?? null}
              clock={correspondence.clock}
              onSelect={(key) => { outboxChecks.open(key); outboxSelection.select(key); }}
              selectedRowRef={outboxSelection.selectedRowRef}
              checkedKeys={outboxChecks.checkedIds}
              onToggleCheck={outboxChecks.toggle}
              onSelectionGesture={outboxChecks.applyGesture}
            />
          </>
          ) : (
            <>
          {loading ? <p className="empty">Loading inbox…</p> : null}
          {mailboxError ? <p className="empty mailbox-error" role="alert">{mailboxError}</p> : null}
          {!loading && threads.length === 0 ? (
            accounts.length === 0 ? (
              <div className="connect-account-cta">
                <Mail size={ICON_SIZE.display} />
                <h2>Connect your first account</h2>
                <p>Add a Gmail account or any IMAP mail server, such as Proton Mail Bridge, to start syncing mail.</p>
                <div className="connect-account-cta-actions">
                  <button type="button" className="btn btn-primary" onClick={() => openSettingsAt("accounts")}>
                    Add Account
                  </button>
                  <button type="button" className="btn" onClick={() => setGettingStartedOpen(true)}>
                    Getting Started
                  </button>
                </div>
              </div>
            ) : query.trim() && remoteSearchState === "searching" ? null : (
              // An empty Main inbox is announced once, centered in the reader.
              inboxZero || mailboxError ? null : (
                <p className="empty">{query.trim() ? "No conversations match your search." : mailbox === "trash" ? "No trashed messages." : mailbox === "sent" ? "No sent messages." : mailbox === "split" ? `No conversations in ${mailboxTitle}.` : "No conversations."}</p>
              )
            )
          ) : null}
          {!loading && threads.length > 0 && visibleThreads.length === 0 ? (
            <div className="empty">
              <p>No conversations match the selected filters.</p>
              <button type="button" className="btn" onClick={clearMessageFilters}>Clear Filters</button>
            </div>
          ) : null}
          {visibleThreads.map((thread) => (
            <ThreadRow
              key={thread.id}
              thread={thread}
              selected={thread.id === selectedId}
              checked={checkedIds.has(thread.id)}
              showAccount={accounts.length > 1}
              accountColor={accountColors.get(thread.accountId)}
              onSelect={selectThreadFromList}
              onToggleCheck={toggleChecked}
              onSelectionGesture={applyThreadSelectionGesture}
              rowRef={thread.id === selectedId ? selectedThreadRowRef : undefined}
              hasTask={openTaskThreadIds.has(thread.id)}
              showLocation={isTabbedMailbox && includeArchived}
            />
          ))}
          {hasMoreResults ? (
            <button className="btn load-more" onClick={() => void loadMoreResults()} disabled={loadingMoreState}>
              {loadingMoreState ? "Loading…" : "Load More Results"}
            </button>
          ) : null}
            </>
          )}
        </ThreadList>
        </ErrorBoundary>
      </section>

      <section className="reader" aria-label="Conversation">
        <ErrorBoundary area="this conversation" resetKey={`${mailbox}:${selectedId ?? ""}:${correspondence.activeDraft?.id ?? ""}`}>
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
                    invitationActions={message.id === invitationMessage?.id ? {
                      calendarConnected,
                      preferences: availabilityPreferences,
                      ai: aiDraftAvailable,
                      intoDraft: Boolean(composerBelongsToVisibleThread && correspondence.liveDraft),
                      onReplyWithTimes: draftAvailabilityReply,
                      onOpenCalendarSettings: () => openSettingsAt("calendarAccounts"),
                    } : undefined}
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
                    emailZoom={emailZoom}
                    emailMinimumFontSize={emailMinimumFontSize}
                    emailTextColors={emailTextColors}
                    threadText={displayedThreadText}
                    onActivate={activateMessage}
                    onToggle={toggleMessage}
                    onRespond={respondToMessage}
                    onEditAsNew={editMessageAsNew}
                    onRegisterNode={registerMessageNode}
                    onImageClick={setLightboxImageSrc}
                    onNotice={showNoticeMessage}
                    revealLink={linkReveal?.messageId === message.id ? linkReveal.reveal : null}
                  />
                );
            }) : null}
            {visibleDetail && !correspondence.activeDraft ? (
              <ThreadDrafts drafts={threadDrafts(correspondence.drafts, visibleDetail.thread)} onOpen={correspondence.openDraft} />
            ) : null}
            {correspondence.renderComposer(context.canSendAndMarkDone ? context.sendAndMarkDone : undefined, {
              enabled: aiDraftFeatureEnabled, available: aiDraftAvailable, onOpenSettings: () => openSettingsAt("ai"),
            })}
          </div>
          </ContactCardContext.Provider>
        ) : mailbox === "outbox" && outboxSelection.selectedEntry ? (
          <OutboxDetail
            key={outboxSelection.selectedEntry.key}
            entry={outboxSelection.selectedEntry}
            clock={correspondence.clock}
            onUndo={correspondence.undoSendItem}
            onRestore={correspondence.restoreFailedSend}
            onReconcile={correspondence.reconcileSend}
            onEditAsNew={correspondence.context.editAsNew}
            pendingActions={correspondence.pendingOutboxActions}
            onChanged={correspondence.refresh}
          >
            {selectedOutboxItem && selectedOutboxMessage ? (
              <ContactCardContext.Provider value={contactCardActions}>
                <div className="message-stack">
                  <MessageCard
                    message={selectedOutboxMessage}
                    index={0}
                    isLatest
                    isExpanded
                    isActive
                    accounts={accounts}
                    queuedItem={selectedOutboxItem}
                    loadRemoteImages={loadRemoteImages}
                    theme={effectiveThemeValue}
                    fontScale={fontScale / 100}
                    fontFamily={fontFamily}
                    emailZoom={emailZoom}
                    emailMinimumFontSize={emailMinimumFontSize}
                    emailTextColors={emailTextColors}
                    onActivate={ignoreOutboxMessageEvent}
                    onToggle={ignoreOutboxMessageEvent}
                    onRespond={ignoreOutboxMessageEvent}
                    onRegisterNode={ignoreOutboxMessageEvent}
                    onImageClick={setLightboxImageSrc}
                    onNotice={showNoticeMessage}
                  />
                </div>
              </ContactCardContext.Provider>
            ) : null}
          </OutboxDetail>
        ) : detailLoading ? (
          <div className="reader-empty" role="status">
            <p>Loading conversation…</p>
          </div>
        ) : inboxZero ? (
          <div className="reader-empty inbox-zero">
            <MailCheck size={ICON_SIZE.display} />
            <p className="inbox-zero-title">Inbox zero</p>
            {inboxChecked ? <p className="inbox-zero-checked">{inboxChecked.text}</p> : null}
            {inboxChecked?.needsReconnect ? (
              <button type="button" className="btn" onClick={() => openSettingsAt("accounts")}>Open Mail Accounts</button>
            ) : null}
          </div>
        ) : isThreadMailbox && !loading && visibleThreads.length === 0 ? (
          // Nothing to select, so the reader gives no instruction to select.
          <div className="reader-empty" />
        ) : (
          <div className="reader-empty">
            <Mail size={ICON_SIZE.display} />
            <p>
              {mailbox === "drafts"
                ? "Select a draft to open it for editing"
                : mailbox === "outbox"
                  ? "No outgoing messages"
                  : "Select a conversation"}
            </p>
          </div>
        )}
        </ErrorBoundary>
      </section>
      </> : null}

      {rightWorkspace === "week" ? (
        <ErrorBoundary area="the calendar" variant="side">
        <Suspense fallback={null}>
          <CalendarWeekView
            session={calendarSession}
            mailAccountIds={activeAccountId ? [activeAccountId] : accounts.map((account) => account.email)}
            onOpenThread={jumpToThread}
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
        </ErrorBoundary>
      ) : null}
      {mailInView && scheduleShown ? (
        <ErrorBoundary area="the calendar" variant="side">
        <CalendarSidebar
          key={calendarSidebarStart?.key ?? 0}
          initialDate={calendarSidebarStart?.date}
          onClose={() => setSideColumn("context")}
          selectedCalendarAccountIds={[...new Set(calendar.calendars.filter((option) => option.selected).map((option) => option.accountId))]}
          onOpenSettings={() => {
            setSideColumn("context");
            openSettingsAt("calendarAccounts");
          }}
        />
        </ErrorBoundary>
      ) : null}
      {mailInView && !scheduleShown && composingApart && correspondence.liveDraft ? (
        <ErrorBoundary area="this draft's context" variant="side" resetKey={correspondence.liveDraft.id}>
        <ComposeContext
          key={correspondence.liveDraft.id}
          draft={correspondence.liveDraft}
          accounts={accounts}
          onKeyDown={contextPanelKeyDown}
          calendarConnected={calendarConnected}
          findTimesAi={aiDraftAvailable}
          preferences={availabilityPreferences}
          taskRefreshKey={taskRevision}
          onAttach={correspondence.context.attachFiles}
          onReplaceRecipient={correspondence.replaceDraftRecipient}
          onSwitchAccount={correspondence.switchDraftAccount}
          onMoveToBcc={correspondence.moveDraftRecipientsToBcc}
          onInsertTimes={draftAvailabilityReply}
          onOpenCalendarSettings={() => openSettingsAt("calendarAccounts")}
          onOpenEvent={(event) => openCalendarAt(eventDate(event))}
          onOpenThread={openThreadFromDraft}
          onShowMessage={(threadId) => openThreadFromDraft(threadId)}
          onEditTask={(task) => setTaskEditor({ kind: "edit", task })}
          onDraftFollowUp={(task) => void draftFollowUp(task)}
          onTasksChanged={() => { setTaskRevision((current) => current + 1); void refreshTaskIndicators(); }}
        />
        </ErrorBoundary>
      ) : null}
      {mailInView && !scheduleShown && !composingApart ? (
        <ErrorBoundary area="the context panel" variant="side" resetKey={selectedId}>
        <ContextPanel
          detail={visibleDetail}
          idle={inboxZero ? (
            <InboxZeroSections onOpenToday={() => executeById("view.today")} />
          ) : null}
          accounts={accounts}
          selectedEmail={contextPersonEmail}
          readingMessageId={readingMessageId}
          reply={composerBelongsToVisibleThread && correspondence.liveDraft ? {
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
          } : null}
          onKeyDown={contextPanelKeyDown}
          sender={senderActions}
          onOpenThread={jumpToThread}
          onShowMessage={showMessage}
          onShowLink={showMessageLink}
          availability={calendarConnected && visibleDetail && !invitationMessage ? (
            // While reading, Find Times reads the conversation, and choosing times starts a reply.
            <AvailabilitySection
              key={visibleDetail.thread.id}
              compact
              preferences={availabilityPreferences}
              ai={aiDraftAvailable}
              draft={composerBelongsToVisibleThread && correspondence.liveDraft
                ? correspondence.liveDraft
                : { sourceId: visibleDetail.messages.at(-1)?.id ?? null, subject: visibleDetail.thread.subject, body: "" }}
              intoDraft={Boolean(composerBelongsToVisibleThread && correspondence.liveDraft)}
              suggest={() => suggestedSearch(actionProposals, new Date(), availabilityPreferences.defaultDurationMinutes)}
              onInsertTimes={draftAvailabilityReply}
              onOpenCalendarSettings={() => openSettingsAt("calendarAccounts")}
            />
          ) : null}
          assist={visibleDetail ? (<>
            <ThreadAssist
              detail={visibleDetail}
              providerChosen={aiProviderChosen}
              summary={{ enabled: aiSummaryFeatureEnabled, available: aiSummaryAvailable, pending: summaryPending }}
              suggestions={{
                enabled: aiActionFeatureEnabled,
                available: aiActionAvailable,
                requested: actionAnalysisRequested,
                proposals: actionProposals,
                hiddenCount: actionHiddenCount,
                extraCount: actionExtraCount,
                onDiscard: discardActionProposal,
                onReview: reviewActionProposal,
              }}
              scheduling={{
                ...meetingScheduling,
                onOpenCalendarEvent: (event) => openCalendarView(eventDate(event), event),
                onAddToCalendar: (_index, proposal, slot) => addMeetingToCalendar(
                  slot,
                  {
                    title: proposal.title,
                    participants: proposal.participants,
                    excerpt: proposal.evidence.excerpt,
                    location: proposal.location,
                    announced: proposal.intent === "attend",
                  },
                  actionProposalSource ? { from: actionProposalSource, proposal } : null,
                ),
              }}
              loading={actionAnalysisLoading}
              error={summaryError ?? actionAnalysisError}
              providerName={aiProviderLabel}
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
                <FindTimesCard
                  key={[availability.rangeStart, availability.rangeEnd, availability.durationMinutes, availability.weekdays.join(","), availability.dayStart, availability.dayEnd].join("|")}
                  plan={planChatAvailability(availability, new Date(), availabilityPreferences.defaultDurationMinutes)}
                  {...meetingScheduling}
                  // A follow-up question adjusts these times; the chat box is already the place to type.
                  ai={null}
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
        </ErrorBoundary>
      ) : null}
      {rightWorkspace === "today" ? <ErrorBoundary area="Today" variant="side" resetKey={activeAccountId}><Suspense fallback={null}><TodayWorkspace
        accountId={activeAccountId}
        refreshKey={syncStatus?.lastSuccessfulSync ?? ""}
        taskRefreshKey={taskRevision}
        calendarConnected={calendarConnected}
        calendars={calendar.calendars}
        onOpenThread={jumpToThread}
        onOpenEvent={(event) => openCalendarView(eventDate(event), event)}
        onEditTask={(task) => setTaskEditor({ kind: "edit", task })}
        onTasksChanged={() => { setTaskRevision((current) => current + 1); void refreshTaskIndicators(); }}
        onOpenInbox={() => executeById("view.mail")}
        onOpenCalendar={() => executeById("view.calendar")}
        onOpenTasks={openTasksView}
        onOpenContact={contactCardActions.onOpenContact}
        onOpenKeepInTouch={openKeepInTouchView}
      /></Suspense></ErrorBoundary> : null}
      {rightWorkspace === "files" ? <ErrorBoundary area="Files" variant="side" resetKey={activeAccountId}><Suspense fallback={null}><FilesWorkspace refreshKey={syncStatus?.lastSuccessfulSync ?? ""} key={activeAccountId ?? "all"} sessions={fileSessions} accountId={activeAccountId} accountIds={accounts.map((account) => account.email)} onOpenThread={jumpToThread} onOpenContact={contactCardActions.onOpenContact} onReportSpam={markFileSourceSpam} /></Suspense></ErrorBoundary> : null}
      {rightWorkspace === "contacts" ? <ErrorBoundary area="Contacts" variant="side" resetKey={activeAccountId}><Suspense fallback={null}><ContactsWorkspace key={`${activeAccountId ?? "all"}:${contactAddressBookTarget ?? ""}`} accountId={activeAccountId} onOpenThread={jumpToThread} onCompose={(to) => { setRightWorkspace(null); correspondence.composeMailto({ to, cc: "", bcc: "", subject: "", body: "" }); }} onSaved={() => { setNotice({ message: "Contact saved" }); void refreshKeepInTouchCount(); }} initialContactId={contactAddressBookTarget} view={contactsView} onViewChange={setContactsView} onKeepInTouchChanged={() => void refreshKeepInTouchCount()} onOpenAiSettings={() => { setRightWorkspace(null); openSettingsAt("ai"); }} /></Suspense></ErrorBoundary> : null}
      {rightWorkspace === "tasks" ? (
        <ErrorBoundary area="Tasks" variant="side" resetKey={activeAccountId}>
        <TaskSidebar
          key={activeAccountId ?? "all"}
          sessions={taskSessions}
          ref={taskWorkspaceRef}
          accountId={activeAccountId}
          accountOptions={accounts.map((account) => account.email)}
          onOpenThread={jumpToThread}
          onUndoableCompletion={(result) => rememberUndo({
            id: "task.complete", title: "Complete Task", keys: [], group: "Triage",
            enabled: () => true, run: async () => result, undo: undoResult,
          }, result, false)}
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
        </ErrorBoundary>
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
          initialLocation={meetingEventDraft.location}
          initialAllDay={meetingEventDraft.allDay}
          sourceMessageId={meetingEventDraft.sourceMessageId}
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
      {muteTarget ? (
        <MuteSenderDialog
          target={muteTarget.target}
          inboxThreadIds={muteTarget.inboxThreadIds}
          onClose={() => setMuteTarget(null)}
          onMute={(matchKind, matchValue, archiveThreadIds) => muteSender(muteTarget.target, matchKind, matchValue, archiveThreadIds)}
        />
      ) : null}
      {splitTarget ? (
        <SenderSplitDialog
          target={splitTarget}
          onClose={() => setSplitTarget(null)}
          onCreate={async (name, matchKind, matchValue, accountId) => {
            await splitInboxCatalog.create(name, matchKind, matchValue, accountId);
            setSplitTarget(null);
            // The new rule takes its matches out of the Inbox now, not at the next sync.
            void loadThreadsRef.current(queryRef.current);
            setNotice({ message: `Created the ${name} split` });
          }}
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
      {gettingStartedOpen ? (
        <GettingStartedDialog
          state={gettingStarted.state}
          onAction={runGettingStartedStep}
          onDismiss={() => { gettingStarted.dismiss(); setGettingStartedOpen(false); }}
          onClose={() => setGettingStartedOpen(false)}
        />
      ) : null}
      {labelTargetIds && labelTargetIds.length > 0 ? (
        <LabelManager
          labels={labelTargetAccountId ? labelsByAccount[labelTargetAccountId] ?? [] : []}
          accountId={labelTargetAccountId}
          capabilities={labelCapabilities(
            accounts.find((account) => account.email === labelTargetAccountId)?.labelModel,
          )}
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
        <ErrorBoundary area="Settings" variant="floating" onDismiss={() => setSettingsOpen(false)}>
        <Suspense fallback={null}>
          <Settings
            section={settingsSection}
            onSectionChange={setSettingsSection}
            onClose={() => setSettingsOpen(false)}
            preferences={preferences}
            mailAccounts={mailAccountSettings}
            calendarAccounts={calendar}
            splitInboxes={splitInboxCatalog}
            mutedSenders={senderRuleCatalog}
            labelsByAccount={labelsByAccount}
            signatures={signatureLibrary}
            snippets={snippetLibrary}
            syncStatus={syncStatus}
            recoveryStatus={recoveryStatus}
            syncDiagnostics={syncDiagnostics}
            onAiConfigChange={refreshAiAvailability}
            onSettingsImported={async (result) => { await applyImportedSettings(result); await signatureLibrary.refresh(); }}
          />
        </Suspense>
        </ErrorBoundary>
      ) : null}
      <EnrollmentRequestNotice
        suppressed={settingsOpen && settingsSection === "replicatedSync"}
        onReview={() => openSettingsAt("replicatedSync")}
      />
      <div className="toast-stack-bottom-left">
        <AvailableAccountsNotice
          accounts={newlyAvailableAccounts}
          suppressed={viewingAccountSettings}
          onReview={() => openSettingsAt("accounts")}
          onDismiss={acknowledgeAvailableAccounts}
        />
        <UpdateNotice />
      </div>
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
          {SERVER_SEARCHING_COPY}
        </div>
      ) : null}
      {isTabbedMailbox && searchOpen && query.trim() && includeArchived && remoteSearchState === "error" ? (
        <div className="toast search-status-toast error" role="status" aria-live="polite">
          <CircleAlert size={ICON_SIZE.sm} />
          {SERVER_SEARCH_FAILED_COPY}
        </div>
      ) : null}
      {isTabbedMailbox && searchOpen && query.trim() && includeArchived && remoteSearchState !== "searching" && searchStatus ? (
        <div className="toast search-status-toast" role="status" aria-live="polite">
          {searchStatus}
          <button className="btn-icon btn-icon-sm" aria-label="Dismiss" onClick={() => setSearchStatus(null)}><X size={ICON_SIZE.sm} /></button>
        </div>
      ) : null}
      {lightboxImageSrc ? (
        <ImageLightbox src={lightboxImageSrc} onClose={() => setLightboxImageSrc(null)} />
      ) : null}
    </main>
  );
}
