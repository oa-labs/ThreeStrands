import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CommandContext } from "./commands";
import { useShortcutHandler } from "./useShortcutHandler";

const context = (overrides: Partial<CommandContext> = {}): CommandContext => ({
  interactionScope: "read",
  focusedPane: "mail",
  mailbox: "inbox",
  selectedId: null,
  selectedArchived: false,
  selectedTrashed: false,
  canUnsubscribe: false,
  canNavigateMessages: false,
  canSendAndMarkDone: false,
  composerActive: false,
  canUndoSend: false,
  splitInboxCount: 0,
  aiSummaryAvailable: false,
  canUndoAction: false,
  compose: vi.fn(),
  reply: vi.fn(),
  replyAll: vi.fn(),
  forward: vi.fn(),
  openInbox: vi.fn(),
  openAllMail: vi.fn(),
  openTrash: vi.fn(),
  openDrafts: vi.fn(),
  openOutbox: vi.fn(),
  openTasks: vi.fn(),
  openMailView: vi.fn(),
  openTasksView: vi.fn(),
  cyclePrimaryView: vi.fn(),
  openActions: vi.fn(),
  newTask: vi.fn(),
  openSplitInbox: vi.fn(),
  goToNextSplitTab: vi.fn(),
  goToPreviousSplitTab: vi.fn(),
  sendDraft: vi.fn(),
  sendAndMarkDone: vi.fn(),
  attachFiles: vi.fn(),
  draftReplyWithAI: vi.fn(),
  undoSend: vi.fn(),
  selectNext: vi.fn(),
  selectPrevious: vi.fn(),
  selectNextTask: vi.fn(),
  selectPreviousTask: vi.fn(),
  openSelectedTask: vi.fn(),
  editSelectedTask: vi.fn(),
  completeSelectedTask: vi.fn(),
  reopenSelectedTask: vi.fn(),
  selectedTaskStatus: null,
  selectNextMessage: vi.fn(),
  selectPreviousMessage: vi.fn(),
  archiveSelected: vi.fn(async () => ({})),
  markNotDoneSelected: vi.fn(async () => ({})),
  unsubscribeSelected: vi.fn(),
  trashSelected: vi.fn(async () => ({})),
  restoreSelected: vi.fn(async () => ({})),
  markSpamSelected: vi.fn(async () => ({})),
  setLabelSelected: vi.fn(async () => ({})),
  toggleReadSelected: vi.fn(async () => ({})),
  toggleStarSelected: vi.fn(async () => ({})),
  toggleCheckedSelected: vi.fn(),
  toggleOlderMessagesExpanded: vi.fn(),
  pageMessageDown: vi.fn(),
  pageMessageUp: vi.fn(),
  summarizeSelected: vi.fn(async () => ({})),
  focusSearch: vi.fn(),
  refresh: vi.fn(),
  openLabels: vi.fn(),
  openPalette: vi.fn(),
  openShortcutHelp: vi.fn(),
  openSettings: vi.fn(),
  openToday: vi.fn(),
  increaseFontSize: vi.fn(),
  decreaseFontSize: vi.fn(),
  undoLastAction: vi.fn(),
  showAllAccounts: vi.fn(),
  switchAccount: vi.fn(),
  toggleMessageFilter: vi.fn(),
  ...overrides,
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("useShortcutHandler", () => {
  it("executes two-step mailbox shortcuts and clears them after a timeout", () => {
    vi.useFakeTimers();
    const current = context();
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "g", cancelable: true }));
    vi.advanceTimersByTime(999);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "i", cancelable: true }));
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "mailbox.inbox" }));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "g", cancelable: true }));
    vi.advanceTimersByTime(1000);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "i", cancelable: true }));
    expect(execute).toHaveBeenCalledTimes(1);
    hook.unmount();
  });

  it("opens the palette with Mod+K and ignores ordinary commands in editable controls", () => {
    const current = context();
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));
    const input = document.createElement("input");
    document.body.append(input);

    input.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true, cancelable: true }));
    expect(execute).not.toHaveBeenCalled();

    input.dispatchEvent(new KeyboardEvent("keydown", { key: "k", metaKey: true, bubbles: true, cancelable: true }));
    expect(current.openPalette).toHaveBeenCalledTimes(1);
    hook.unmount();
  });

  it("uses current context values and stops responding after unmount", () => {
    const first = context();
    const second = context();
    const execute = vi.fn();
    const hook = renderHook(({ value }) => useShortcutHandler(value, execute), { initialProps: { value: first } });
    hook.rerender({ value: second });

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true, cancelable: true }));
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "draft.new" }));
    hook.unmount();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true, cancelable: true }));
    expect(execute).toHaveBeenCalledTimes(1);
  });

  it("respects explicit entry scopes and preserves native button activation", () => {
    const current = context();
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));
    const modal = document.createElement("div");
    modal.dataset.shortcutScope = "modal";
    const modalButton = document.createElement("button");
    modal.append(modalButton);
    document.body.append(modal);

    modalButton.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true, cancelable: true }));
    modalButton.dispatchEvent(new KeyboardEvent("keydown", { key: " ", code: "Space", bubbles: true, cancelable: true }));
    expect(execute).not.toHaveBeenCalled();

    const readButton = document.createElement("button");
    document.body.append(readButton);
    readButton.dispatchEvent(new KeyboardEvent("keydown", { key: " ", code: "Space", bubbles: true, cancelable: true }));
    expect(execute).not.toHaveBeenCalled();
    hook.unmount();
  });
});
