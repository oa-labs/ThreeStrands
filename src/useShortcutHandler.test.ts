import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CommandContext } from "./commands";
import { useEscapeDismiss } from "./useEscapeDismiss";
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
  canGoBack: false,
  goBack: vi.fn(),
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
  openContactsView: vi.fn(),
  openKeepInTouchView: vi.fn(),
  openContactGroupsView: vi.fn(),
  openCalendarView: vi.fn(),
  getSuggestions: vi.fn(),
  openThreadChat: vi.fn(),
  newTask: vi.fn(),
  openSplitInbox: vi.fn(),
  goToNextSplitTab: vi.fn(),
  goToPreviousSplitTab: vi.fn(),
  sendDraft: vi.fn(),
  sendAndMarkDone: vi.fn(),
  attachFiles: vi.fn(),
  discardDraft: vi.fn(),
  draftReplyWithAI: vi.fn(),
  toggleContextPanelFocus: vi.fn(),
  undoSend: vi.fn(),
  selectNext: vi.fn(),
  selectPrevious: vi.fn(),
  selectNextTask: vi.fn(),
  selectPreviousTask: vi.fn(),
  openSelectedTask: vi.fn(),
  openTaskDetails: vi.fn(),
  focusGoals: vi.fn(),
  linkTaskToGoal: vi.fn(),
  completeSelectedTask: vi.fn(),
  reopenSelectedTask: vi.fn(),
  selectedTaskStatus: null,
  moveSelectedTask: vi.fn(),
  selectAdjacentTaskColumn: vi.fn(),
  toggleTaskLayout: vi.fn(),
  cycleTaskView: vi.fn(),
  cycleContactsView: vi.fn(),
  taskBoardActive: false,
  calendarWeekActive: false,
  selectedTaskHasThread: false,
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

  it("leaves Escape to an open overlay, and otherwise runs Go Back", () => {
    const current = context({ canGoBack: true });
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));
    const overlay = renderHook(() => useEscapeDismiss(() => {}));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(execute).not.toHaveBeenCalled();

    overlay.unmount();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "navigation.back" }));
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
    // The first context is composing, so the read-scope `c` shortcut is suppressed.
    const first = context({ interactionScope: "compose", composerActive: true });
    const second = context();
    const execute = vi.fn();
    const hook = renderHook(({ value }) => useShortcutHandler(value, execute), { initialProps: { value: first } });
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true, cancelable: true }));
    expect(execute).not.toHaveBeenCalled();

    hook.rerender({ value: second });

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true, cancelable: true }));
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "draft.new" }));
    hook.unmount();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "c", bubbles: true, cancelable: true }));
    expect(execute).toHaveBeenCalledTimes(1);
  });

  it("respects explicit entry scopes and preserves native button activation", () => {
    // A selected thread enables Space (page down), so the guards below are
    // what keep it from firing, not a disabled command.
    const current = context({ selectedId: "thread-1" });
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

    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: " ", code: "Space", bubbles: true, cancelable: true }));
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "thread.pageDown" }));
    modal.remove();
    readButton.remove();
    hook.unmount();
  });

  it("lets # discard the open draft in Drafts unless focus is in an editable field", () => {
    const current = context({ interactionScope: "compose", composerActive: true, mailbox: "drafts" });
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));
    const composer = document.createElement("div");
    composer.className = "composer";
    composer.dataset.shortcutScope = "compose";
    const subject = document.createElement("input");
    const body = document.createElement("textarea");
    const button = document.createElement("button");
    composer.append(subject, body, button);
    document.body.append(composer);

    for (const target of [subject, body]) {
      const typed = new KeyboardEvent("keydown", { key: "#", code: "Digit3", shiftKey: true, bubbles: true, cancelable: true });
      target.dispatchEvent(typed);
      expect(typed.defaultPrevented).toBe(false);
    }
    expect(execute).not.toHaveBeenCalled();

    button.dispatchEvent(new KeyboardEvent("keydown", { key: "#", code: "Digit3", shiftKey: true, bubbles: true, cancelable: true }));
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "draft.discard" }));
    hook.unmount();
  });

  it("switches to the context panel from inside the draft's fields, and only while composing", () => {
    const execute = vi.fn();
    let current = context({ interactionScope: "compose", composerActive: true });
    const hook = renderHook(() => useShortcutHandler(current, execute));
    const composer = document.createElement("div");
    composer.className = "composer";
    composer.dataset.shortcutScope = "compose";
    const to = document.createElement("input");
    const body = document.createElement("div");
    body.contentEditable = "true";
    composer.append(to, body);
    const panel = document.createElement("aside");
    const fix = document.createElement("button");
    panel.append(fix);
    document.body.append(composer, panel);

    for (const [target, init] of [
      [to, { key: "F6" }],
      [body, { key: "P", metaKey: true, shiftKey: true }],
      [fix, { key: "F6" }],
    ] as const) {
      const event = new KeyboardEvent("keydown", { ...init, bubbles: true, cancelable: true });
      target.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
    }
    expect(execute).toHaveBeenCalledTimes(3);
    expect(execute).toHaveBeenLastCalledWith(expect.objectContaining({ id: "draft.contextPanel" }));

    execute.mockClear();
    current = context();
    hook.rerender();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "F6", cancelable: true }));
    expect(execute).not.toHaveBeenCalled();
    composer.remove();
    panel.remove();
    hook.unmount();
  });

  it("keeps # inert in an open composer outside the Drafts folder", () => {
    const current = context({ interactionScope: "compose", composerActive: true, mailbox: "inbox", selectedId: "thread-1" });
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "#", code: "Digit3", shiftKey: true, cancelable: true }));
    expect(execute).not.toHaveBeenCalled();
    hook.unmount();
  });
});
