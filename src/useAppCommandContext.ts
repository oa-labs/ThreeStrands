import { runWorkspaceCommand } from "./workspaceCommands";
import { useMemo, type Dispatch, type SetStateAction } from "react";
import type { CommandContext, CommandResult, MailboxKind } from "./commands";
import type { Message } from "./domain";
import type { MutationTemplate } from "./threadMutations";
import { adjacentContactsView } from "./contactsView";
import { selectedMessageQuote } from "./selectedMessageQuote";
import { editAsNewSource } from "./editAsNew";
import { outboxEditAsNewSource } from "./outboxView";
import { saveSelectedMailboxForAccount } from "./settings";
import { scrollBehavior } from "./useReaderActions";
import type { SettingsSection } from "./settingsPanelTypes";
import type { useCorrespondence } from "./useCorrespondence";
import type { useReaderState } from "./useReaderState";
import type { useWorkspaces } from "./useWorkspaces";
import type { useMailNavigation } from "./useMailNavigation";
import type { useThreadSelection } from "./useThreadSelection";
import type { useTaskActions } from "./useTaskActions";
import type { useCommandUndo } from "./useCommandExecution";
import type { useMailboxThreads } from "./useMailboxThreads";
import type { useTriageSession } from "./useTriageSession";
import type { useThreadIntelligence } from "./useThreadIntelligence";
import type { useOutboxSelection } from "./useOutboxSelection";

type Options = {
  correspondence: ReturnType<typeof useCorrespondence>;
  reader: ReturnType<typeof useReaderState>;
  workspaces: ReturnType<typeof useWorkspaces>;
  navigation: ReturnType<typeof useMailNavigation>;
  selection: ReturnType<typeof useThreadSelection>;
  tasks: ReturnType<typeof useTaskActions>;
  undo: ReturnType<typeof useCommandUndo>;
  mailboxThreads: ReturnType<typeof useMailboxThreads>;
  runBrief: ReturnType<typeof useThreadIntelligence>["runBrief"];
  recordTriageEvent: ReturnType<typeof useTriageSession>["recordTriageEvent"];
  displayedMessages: Message[];
  composerBelongsToVisibleThread: boolean;
  outboxSelection: ReturnType<typeof useOutboxSelection>;
  mutateIds: (ids: string[], template: MutationTemplate, options?: { includeUnloaded?: boolean }) => Promise<CommandResult>;
  view: {
    mailbox: MailboxKind;
    setMailbox: Dispatch<SetStateAction<MailboxKind>>;
    setActiveSplitInboxId: Dispatch<SetStateAction<string | null>>;
    activeAccountId: string | null;
    selectedId: string | null;
    setSelectedId: Dispatch<SetStateAction<string | null>>;
    isTabbedMailbox: boolean;
  };
  ui: {
    interactionScope: CommandContext["interactionScope"];
    canUnsubscribe: boolean;
    latestMessage: Message | null;
    aiSummaryAvailable: boolean;
    /** Briefs or suggestions are on with a provider chosen. */
    aiBriefEnabled: boolean;
    /** Thread Chat is on with a provider chosen. */
    aiChatEnabled: boolean;
    /** Draft Assist is on with a provider chosen. */
    aiDraftEnabled: boolean;
    labelTargetIds: string[] | null;
    setLabelTargetIds: Dispatch<SetStateAction<string[] | null>>;
    setUnsubscribeMessageId: Dispatch<SetStateAction<string | null>>;
    setPaletteOpen: Dispatch<SetStateAction<boolean>>;
    setShortcutHelpOpen: Dispatch<SetStateAction<boolean>>;
    setGettingStartedOpen: Dispatch<SetStateAction<boolean>>;
    openSettingsAt: (section?: SettingsSection) => void;
    adjustFontScale: (direction: 1 | -1) => void;
    toggleContextPanelFocus: () => void;
    selectAdjacentMessage: (direction: -1 | 1) => void;
    getSuggestions: () => void;
    openThreadChat: () => void;
    refreshMail: () => void;
  };
};

/** Binds global commands to the current domain controllers and interaction scope. */
export function useAppCommandContext({
  correspondence, reader, workspaces, navigation, selection, tasks, undo, mailboxThreads, runBrief,
  recordTriageEvent, displayedMessages, composerBelongsToVisibleThread, outboxSelection, mutateIds, view, ui,
}: Options) {
  const outboxEditSource = useMemo(() => outboxEditAsNewSource(outboxSelection.selectedEntry), [outboxSelection.selectedEntry]);
  const { messageStackRef, setMessageExpansionOverrides, activeMessageIdRef } = reader;
  const {
    rightWorkspace, setRightWorkspace, setContactsView, openToday, openTasks, openTasksView,
    openContactsView, openKeepInTouchView, openContactGroupsView, openCalendarView, openFilesView,
  } = workspaces;
  const {
    accountSplitInboxes, returnStep, goBack, goToInboxTab, openMailView, goToSplitTab,
    goToNextSplitTab, goToPreviousSplitTab, openFolder, switchAccount,
  } = navigation;
  const { selected, selectedIndex, visibleThreads, toggleChecked, toggleMessageFilter, checkedIds, setCheckedIds } = selection;
  const targetIds = useMemo(() => checkedIds.size > 0 ? [...checkedIds] : selected ? [selected.id] : [], [checkedIds, selected]);
  const targetThreads = useMemo(() => mailboxThreads.threads.filter((thread) => targetIds.includes(thread.id)), [mailboxThreads.threads, targetIds]);
  const { taskWorkspaceRef, taskLayout, selectedTaskStatus, selectedTaskHasThread, newTask } = tasks;
  const { canUndoAction, canUndoArchive, undoLastAction } = undo;
  const { includeArchived, setSearchOpen } = mailboxThreads;
  const { mailbox, setMailbox, setActiveSplitInboxId, activeAccountId, selectedId, setSelectedId, isTabbedMailbox } = view;
  const {
    interactionScope, canUnsubscribe, latestMessage, aiSummaryAvailable, aiBriefEnabled, aiChatEnabled, aiDraftEnabled, labelTargetIds,
    setLabelTargetIds, setUnsubscribeMessageId, setPaletteOpen, setShortcutHelpOpen, setGettingStartedOpen,
    openSettingsAt, adjustFontScale, toggleContextPanelFocus, selectAdjacentMessage,
    getSuggestions, openThreadChat, refreshMail,
  } = ui;
  const context = useMemo<CommandContext>(() => ({
    ...correspondence.context,
    toggleContextPanelFocus,
    interactionScope,
    focusedPane: rightWorkspace === "files" ? "files" : rightWorkspace === "week" ? "calendar" : rightWorkspace === "tasks" ? "tasks" : rightWorkspace === "contacts" ? "contacts" : "mail",
    compose: () => {
      setRightWorkspace(null);
      correspondence.context.compose();
    },
    mailbox,
    selectedId,
    checkedCount: checkedIds.size,
    selectAllConversations: () => setCheckedIds(new Set(visibleThreads.map((thread) => thread.id))),
    selectedArchived: selected?.archived ?? false,
    selectedTrashed: selected?.trashed ?? false,
    canUnsubscribe,
    canGoBack: returnStep !== null,
    goBack,
    canNavigateMessages: displayedMessages.length > 1,
    canSendAndMarkDone: composerBelongsToVisibleThread,
    sendAndMarkDone: () => {
      if (!selected || !composerBelongsToVisibleThread) return;
      const threadId = selected.id;
      correspondence.context.sendDraftAndThen(() => {
        void mutateIds([threadId], { kind: "archive", value: true });
      }, true);
    },
    openInbox: goToInboxTab,
    splitInboxCount: accountSplitInboxes.length,
    goToNextSplitTab,
    goToPreviousSplitTab,
    openAllMail: () => openFolder("allMail"),
    openSent: () => openFolder("sent"),
    openTrash: () => openFolder("trash"),
    openSplitInbox: goToSplitTab,
    openDrafts: () => openFolder("drafts"),
    openOutbox: () => openFolder("outbox"),
    selectNext: () => {
      if (mailbox === "outbox") { outboxSelection.selectAdjacent(1); return; }
      const next = Math.min(selectedIndex + 1, visibleThreads.length - 1);
      setSelectedId(visibleThreads[next]?.id ?? null);
    },
    selectPrevious: () => {
      if (mailbox === "outbox") { outboxSelection.selectAdjacent(-1); return; }
      const next = Math.max(selectedIndex - 1, 0);
      setSelectedId(visibleThreads[next]?.id ?? null);
    },
    selectNextMessage: () => selectAdjacentMessage(1),
    selectPreviousMessage: () => selectAdjacentMessage(-1),
    selectNextTask: () => taskWorkspaceRef.current?.selectNext(),
    selectPreviousTask: () => taskWorkspaceRef.current?.selectPrevious(),
    openSelectedTask: () => taskWorkspaceRef.current?.openSelected(),
    openTaskDetails: () => taskWorkspaceRef.current?.openDetails(),
    completeSelectedTask: () => taskWorkspaceRef.current?.completeSelected(),
    reopenSelectedTask: () => taskWorkspaceRef.current?.reopenSelected(),
    moveSelectedTask: (direction) => taskWorkspaceRef.current?.moveSelected(direction),
    selectAdjacentTaskColumn: (direction) => taskWorkspaceRef.current?.selectAdjacentColumn(direction),
    toggleTaskLayout: () => taskWorkspaceRef.current?.toggleLayout(),
    cycleTaskView: (direction) => taskWorkspaceRef.current?.cycleView(direction),
    cycleContactsView: (direction) => setContactsView((current) => adjacentContactsView(current, direction)),
    focusGoals: () => taskWorkspaceRef.current?.focusGoals(),
    linkTaskToGoal: () => taskWorkspaceRef.current?.linkSelectedToGoal(),
    taskBoardActive: rightWorkspace === "tasks" && taskLayout === "board",
    calendarWeekActive: rightWorkspace === "week",
    selectedTaskStatus,
    selectedTaskHasThread,
    archiveSelected: () => mutateIds(targetIds, { kind: "archive", value: true }),
    markNotDoneSelected: () => mutateIds(selected ? [selected.id] : [], { kind: "archive", value: false }),
    unsubscribeSelected: () => {
      if (latestMessage?.unsubscribe?.methods.length) setUnsubscribeMessageId(latestMessage.id);
    },
    trashSelected: () => mutateIds(targetIds, { kind: "trash", value: true }),
    restoreSelected: async () => {
      if (!selected) return {};
      // Matches Gmail: restoring from Trash always lands back in the Inbox,
      // regardless of whether the thread was archived before it was trashed.
      const wasArchived = selected.archived;
      const result = await mutateIds([selected.id], { kind: "trash", value: false });
      if (wasArchived) await mutateIds([selected.id], { kind: "archive", value: false });
      return result;
    },
    markSpamSelected: () => mutateIds(targetIds, { kind: "spam", value: true }),
    setLabelSelected: (labelId, labelName, value) =>
      mutateIds(labelTargetIds ?? [], { kind: "label", labelId, labelName, value }),
    toggleReadSelected: () =>
      mutateIds(targetIds, { kind: "read", value: targetThreads.some((thread) => thread.unread) }),
    toggleStarSelected: () =>
      mutateIds(targetIds, { kind: "star", value: !targetThreads.every((thread) => thread.starred) }),
    reply: () => {
      if (selected) {
        recordTriageEvent({
          threadId: selected.id,
          kind: "response",
          context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
        });
      }
      const quote = selectedMessageQuote();
      correspondence.context.reply(quote?.messageId, quote?.text);
    },
    replyAll: () => {
      if (selected) {
        recordTriageEvent({
          threadId: selected.id,
          kind: "response",
          context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
        });
      }
      const quote = selectedMessageQuote();
      correspondence.context.replyAll(quote?.messageId, quote?.text);
    },
    forward: () => {
      if (selected) {
        recordTriageEvent({
          threadId: selected.id,
          kind: "response",
          context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
        });
      }
      const quote = selectedMessageQuote();
      correspondence.context.forward(quote?.messageId, quote?.text);
    },
    canEditAsNew: mailbox === "outbox"
      ? outboxEditSource !== null
      : editAsNewSource(displayedMessages, selected?.accountId) !== null,
    editAsNew: () => {
      if (mailbox === "outbox") {
        if (outboxEditSource) correspondence.context.editAsNew(outboxEditSource.messageId, outboxEditSource.account);
        return;
      }
      const source = editAsNewSource(displayedMessages, selected?.accountId, activeMessageIdRef.current);
      if (source) correspondence.context.editAsNew(source.id);
    },
    toggleCheckedSelected: () => {
      if (selected) toggleChecked(selected.id);
    },
    toggleOlderMessagesExpanded: () => setMessageExpansionOverrides((current) => {
      const olderMessages = displayedMessages.slice(0, -1);
      const allExpanded = olderMessages.every((message) => current.get(message.id) ?? message.unread);
      return new Map(olderMessages.map((message) => [message.id, !allExpanded]));
    }),
    pageMessageDown: () => {
      const node = messageStackRef.current;
      if (!node) return;
      node.scrollBy({ top: node.clientHeight * 0.9, behavior: scrollBehavior() });
    },
    pageMessageUp: () => {
      const node = messageStackRef.current;
      if (!node) return;
      node.scrollBy({ top: -node.clientHeight * 0.9, behavior: scrollBehavior() });
    },
    aiSummaryAvailable,
    aiBriefEnabled,
    aiChatEnabled,
    aiReplyAssistEnabled: aiDraftEnabled && (correspondence.activeDraft?.mode === "reply" || correspondence.activeDraft?.mode === "replyAll"),
    summarizeSelected: async () => {
      if (!selected) return {};
      setRightWorkspace((current) => current === "calendar" ? current : null);
      await runBrief({ only: "summary" });
      return {};
    },
    focusSearch: () => {
      if (rightWorkspace === "files") { runWorkspaceCommand("files.search"); return; }
      if (rightWorkspace === "contacts") { runWorkspaceCommand("contacts.search"); return; }
      setRightWorkspace(null);
      if (!isTabbedMailbox) {
        correspondence.context.openInbox();
        setMailbox("inbox");
        saveSelectedMailboxForAccount(activeAccountId, "inbox");
        setActiveSplitInboxId(null);
      }
      setSearchOpen(true);
    },
    refresh: refreshMail,
    openLabels: () => setLabelTargetIds(targetIds.length ? targetIds : null),
    openPalette: () => setPaletteOpen(true),
    openShortcutHelp: () => setShortcutHelpOpen(true),
    openGettingStarted: () => setGettingStartedOpen(true),
    openSettings: () => openSettingsAt(),
    openToday,
    openTasks,
    openMailView,
    openTasksView,
    openFilesView,
    openContactsView,
    openKeepInTouchView,
    openContactGroupsView,
    openCalendarView,
    getSuggestions,
    openThreadChat,
    newTask,
    increaseFontSize: () => adjustFontScale(1),
    decreaseFontSize: () => adjustFontScale(-1),
    canUndoAction,
    canUndoArchive,
    undoLastAction: () => {
      if (correspondence.context.canUndoSend && (!canUndoAction || correspondence.pendingUndoOrder >= undo.lastUndoOrder)) correspondence.context.undoSend();
      else void undoLastAction();
    },
    switchAccount,
    showAllAccounts: () => switchAccount(null),
    toggleMessageFilter,
  }), [
    correspondence.context, correspondence.pendingUndoOrder, undo.lastUndoOrder, checkedIds, setCheckedIds, targetIds, targetThreads, toggleContextPanelFocus, interactionScope, rightWorkspace, mailbox,
    selectedId, selected, canUnsubscribe, returnStep, goBack, displayedMessages,
    composerBelongsToVisibleThread, outboxSelection, outboxEditSource, goToInboxTab, accountSplitInboxes.length, goToNextSplitTab,
    goToPreviousSplitTab, goToSplitTab, taskLayout, selectedTaskStatus, selectedTaskHasThread,
    aiSummaryAvailable, aiBriefEnabled, aiChatEnabled, aiDraftEnabled, correspondence.activeDraft?.mode, refreshMail, openToday, openTasks, openMailView, openTasksView,
    openContactsView, openKeepInTouchView, openContactGroupsView, openCalendarView, openFilesView, getSuggestions,
    openThreadChat, newTask, canUndoAction, canUndoArchive, switchAccount, toggleMessageFilter, setRightWorkspace,
    mutateIds, openFolder, selectedIndex, visibleThreads, setSelectedId, selectAdjacentMessage,
    taskWorkspaceRef, setContactsView, latestMessage, setUnsubscribeMessageId, labelTargetIds,
    recordTriageEvent, includeArchived, toggleChecked, setMessageExpansionOverrides,
    messageStackRef, activeMessageIdRef, runBrief, isTabbedMailbox, setSearchOpen, setMailbox, activeAccountId,
    setActiveSplitInboxId, setLabelTargetIds, setPaletteOpen, setShortcutHelpOpen, setGettingStartedOpen, openSettingsAt,
    adjustFontScale, undoLastAction,
  ]);

  return context;
}
