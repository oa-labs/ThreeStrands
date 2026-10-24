import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { commandContext as context } from "./test/commandContext";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { useShortcutHandler } from "./useShortcutHandler";


afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("useShortcutHandler", () => {
  it("attaches files from compose fields with either modifier, suppressing overlays, IME, and held keys", () => {
    const execute = vi.fn();
    renderHook(() => useShortcutHandler(context({ composerActive: true, interactionScope: "compose" }), execute));
    const composer = document.createElement("div"); composer.className = "composer"; composer.dataset.shortcutScope = "compose";
    const field = document.createElement("textarea"); composer.append(field); document.body.append(composer);
    for (const modifier of [{ metaKey: true }, { ctrlKey: true }]) field.dispatchEvent(new KeyboardEvent("keydown", { key: "A", shiftKey: true, bubbles: true, ...modifier }));
    expect(execute).toHaveBeenCalledTimes(2);
    expect(execute).toHaveBeenLastCalledWith(expect.objectContaining({ id: "draft.attach" }));
    for (const extra of [{ repeat: true }, { isComposing: true }, { altKey: true }]) field.dispatchEvent(new KeyboardEvent("keydown", { key: "A", shiftKey: true, metaKey: true, bubbles: true, ...extra }));
    field.dataset.shortcutScope = "modal";
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "A", shiftKey: true, metaKey: true, bubbles: true }));
    expect(execute).toHaveBeenCalledTimes(2);
  });

  it("saves Contacts from inputs and focused buttons but preserves modal and editor Undo", () => {
    const execute = vi.fn();
    renderHook(() => useShortcutHandler(context({ focusedPane: "contacts", canUndoAction: true, canUndoSend: true }), execute));
    const field = document.createElement("input"); const button = document.createElement("button"); document.body.append(field, button);
    for (const target of [field, button]) target.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
    expect(execute.mock.calls.map(([command]) => command.id)).toEqual(["contacts.save", "contacts.save"]);
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "z", metaKey: true, bubbles: true }));
    field.dataset.shortcutScope = "modal";
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
    expect(execute).toHaveBeenCalledTimes(2);
  });

  it("selects all from the mail list and body, preserving text selection in readers and editing fields", () => {
    const execute = vi.fn();
    renderHook(() => useShortcutHandler(context({}), execute));
    const list = document.createElement("div"); list.className = "thread-list";
    const row = document.createElement("button"); list.append(row);
    const reader = document.createElement("div"); reader.className = "message-card";
    const field = document.createElement("input"); document.body.append(list, reader, field);
    for (const target of [document.body, row]) target.dispatchEvent(new KeyboardEvent("keydown", { key: "a", ctrlKey: true, bubbles: true }));
    for (const target of [reader, field, window]) target.dispatchEvent(new KeyboardEvent("keydown", { key: "a", metaKey: true, bubbles: true }));
    expect(execute.mock.calls.map(([command]) => command.id)).toEqual(["selection.all", "selection.all"]);
  });

  it("refreshes mail only outside editing, search, and modal contexts", () => {
    const execute = vi.fn();
    renderHook(() => useShortcutHandler(context({}), execute));
    const field = document.createElement("input"); document.body.append(field);
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "r", ctrlKey: true, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "r", metaKey: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "r", metaKey: true, repeat: true }));
    expect(execute).toHaveBeenCalledOnce();
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "mail.refresh" }));
  });

  it("toggles light and dark mode with Mod+D anywhere, including text fields and the composer, but not on held keys", () => {
    const execute = vi.fn();
    renderHook(() => useShortcutHandler(context({ composerActive: true, interactionScope: "compose" }), execute));
    const composer = document.createElement("div"); composer.className = "composer"; composer.dataset.shortcutScope = "compose";
    const draft = document.createElement("textarea"); composer.append(draft);
    const search = document.createElement("input"); search.dataset.shortcutScope = "search";
    document.body.append(composer, search);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "d", metaKey: true }));
    draft.dispatchEvent(new KeyboardEvent("keydown", { key: "d", ctrlKey: true, bubbles: true }));
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "D", metaKey: true, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "d", metaKey: true, repeat: true }));
    for (const extra of [{ shiftKey: true }, { altKey: true }, { isComposing: true }]) window.dispatchEvent(new KeyboardEvent("keydown", { key: "d", metaKey: true, ...extra }));
    draft.dispatchEvent(new KeyboardEvent("keydown", { key: "d", bubbles: true }));
    expect(execute.mock.calls.map(([command]) => command.id)).toEqual(["theme.toggle", "theme.toggle", "theme.toggle"]);
  });

  it("opens Send later from compose fields with Mod+Shift+L and respects overlays, IME, and repeats", () => {
    const current = context({ interactionScope: "compose", composerActive: true });
    const execute = vi.fn();
    renderHook(() => useShortcutHandler(current, execute));
    const composer = document.createElement("div");
    composer.className = "composer";
    composer.dataset.shortcutScope = "compose";
    const subject = document.createElement("input");
    const body = document.createElement("div");
    body.contentEditable = "true";
    const modal = document.createElement("div");
    modal.dataset.shortcutScope = "modal";
    const modalInput = document.createElement("input");
    modal.append(modalInput);
    composer.append(subject, body, modal);
    document.body.append(composer);
    for (const field of [subject, body]) {
      for (const modifier of [{ metaKey: true }, { ctrlKey: true }]) {
        const event = new KeyboardEvent("keydown", { key: "L", shiftKey: true, ...modifier, bubbles: true, cancelable: true });
        field.dispatchEvent(event);
        expect(event.defaultPrevented).toBe(true);
      }
    }
    expect(execute).toHaveBeenCalledTimes(4);
    expect(execute.mock.calls.every(([command]) => command.id === "draft.schedule")).toBe(true);
    for (const extra of [{ repeat: true }, { isComposing: true }, { shiftKey: false }, { altKey: true }]) {
      body.dispatchEvent(new KeyboardEvent("keydown", { key: "L", metaKey: true, shiftKey: true, bubbles: true, ...extra }));
    }
    modalInput.dispatchEvent(new KeyboardEvent("keydown", { key: "L", metaKey: true, shiftKey: true, bubbles: true }));
    expect(execute).toHaveBeenCalledTimes(4);
  });

  it("keeps Shift+E out of text fields and overlays, and ignores held-key repeats", () => {
    const current = context({ canUndoArchive: true });
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));
    const input = document.createElement("input");
    document.body.append(input);
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true, repeat: true }));
    expect(execute).not.toHaveBeenCalled();

    const modal = document.createElement("div");
    modal.dataset.shortcutScope = "modal";
    document.body.append(modal);
    modal.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true, bubbles: true }));
    expect(execute).not.toHaveBeenCalled();

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true }));
    expect(execute).toHaveBeenCalledOnce();
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "thread.unarchive" }));
    hook.unmount();
  });

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

  it("ignores key repeat for one-shot actions so holding e archives only the selected thread", () => {
    const current = context({ selectedId: "thread-1" });
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "e", cancelable: true }));
    const repeat = new KeyboardEvent("keydown", { key: "e", repeat: true, cancelable: true });
    window.dispatchEvent(repeat);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "#", repeat: true, cancelable: true }));

    expect(execute).toHaveBeenCalledTimes(1);
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "thread.archive" }));
    expect(repeat.defaultPrevented).toBe(true);
    hook.unmount();
  });

  it("lets navigation commands repeat while a key is held", () => {
    const current = context({ selectedId: "thread-1" });
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "j", cancelable: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "j", repeat: true, cancelable: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", repeat: true, cancelable: true }));

    expect(execute.mock.calls.map(([command]) => command.id)).toEqual(["thread.next", "thread.next", "thread.previous"]);
    hook.unmount();
  });

  it("does not complete a two-step shortcut from a held prefix key", () => {
    const current = context({ focusedPane: "tasks" });
    const execute = vi.fn();
    const hook = renderHook(() => useShortcutHandler(current, execute));

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "g", cancelable: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "g", repeat: true, cancelable: true }));
    expect(execute).not.toHaveBeenCalled();
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
