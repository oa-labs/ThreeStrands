import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useRef, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CommandContext } from "./commands";
import { ActionButton, CommandPalette, FiltersButton, Modal, ShortcutHelp } from "./AppChrome";

afterEach(cleanup);

const context = {
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
} as unknown as CommandContext;

describe("App chrome", () => {
  it("opens filters, reports the selected filter, and closes on Escape", () => {
    const onToggleFilter = vi.fn();
    render(<FiltersButton activeFilters={new Set(["starred"])} onToggleFilter={onToggleFilter} />);

    fireEvent.click(screen.getByRole("button", { name: /Filters/ }));
    expect(screen.getByRole("menu", { name: "Filters" })).toBeInTheDocument();
    expect(screen.getByRole("menuitemcheckbox", { name: /Starred/ })).toHaveAttribute("aria-checked", "true");

    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: /Unread/ }));
    expect(onToggleFilter).toHaveBeenCalledWith("unread");

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("menu", { name: "Filters" })).not.toBeInTheDocument();
  });

  it("filters the command palette and executes a command", () => {
    const execute = vi.fn();
    const onClose = vi.fn();
    render(<CommandPalette context={context} execute={execute} onClose={onClose} />);

    const input = screen.getByRole("textbox", { name: "Filter Commands" });
    fireEvent.change(input, { target: { value: "new message" } });
    const newMessage = screen.getByRole("button", { name: /New Message/ });
    expect(newMessage).toBeEnabled();

    fireEvent.click(newMessage);
    expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "draft.new" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("renders shortcut help and reusable action buttons", () => {
    const action = vi.fn();
    const close = vi.fn();
    const view = render(<ActionButton label="Refresh" shortcut="r" onClick={action}>↻</ActionButton>);

    fireEvent.click(screen.getByRole("button", { name: "Refresh (r)" }));
    expect(action).toHaveBeenCalledTimes(1);
    view.rerender(<ShortcutHelp onClose={close} />);
    expect(screen.getByRole("dialog", { name: "Keyboard Shortcuts" })).toBeInTheDocument();
    expect(screen.getByText("Use ThreeStrands without leaving the keyboard.")).toBeInTheDocument();
  });

  it("traps modal focus, makes the background inert, and restores the trigger", () => {
    function Harness() {
      const [open, setOpen] = useState(false);
      const inputRef = useRef<HTMLInputElement>(null);
      return <><button type="button" onClick={() => setOpen(true)}>Open editor</button>{open ? <Modal title="Editor" onClose={() => setOpen(false)} initialFocusRef={inputRef}><input ref={inputRef} aria-label="Editor value" /><button type="button">Last action</button></Modal> : null}</>;
    }
    const { container } = render(<Harness />);
    const trigger = screen.getByRole("button", { name: "Open editor" });
    trigger.focus();
    fireEvent.click(trigger);

    expect(screen.getByRole("textbox", { name: "Editor value" })).toHaveFocus();
    expect(container).toHaveAttribute("aria-hidden", "true");
    const last = screen.getByRole("button", { name: "Last action" });
    last.focus();
    fireEvent.keyDown(last, { key: "Tab" });
    expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Editor" })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    expect(container).not.toHaveAttribute("aria-hidden");
  });

  it("keeps a non-dismissible modal open and stops Escape reaching the modal underneath", () => {
    const closeOuter = vi.fn();
    const closeInner = vi.fn();
    render(
      <>
        <Modal title="Outer" onClose={closeOuter}><button type="button">Outer action</button></Modal>
        <Modal title="Inner" dismissible={false} onClose={closeInner}><button type="button">Only way out</button></Modal>
      </>,
    );
    // Both mounted in one commit, as when Settings reopens with a
    // recovery phrase still pending: the later modal stays reachable.
    const inner = screen.getByRole("dialog", { name: "Inner" });
    expect(inner.parentElement).not.toHaveAttribute("aria-hidden");
    expect((inner.parentElement as HTMLElement).inert).toBeFalsy();

    expect(inner.querySelector("header button")).toBeNull();
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.mouseDown(inner.parentElement!);
    expect(closeInner).not.toHaveBeenCalled();
    expect(closeOuter).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Inner" })).toBeInTheDocument();
  });
});
