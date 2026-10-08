import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useRef, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CommandContext } from "./commands";
import { ActionButton, CommandPalette, FiltersButton, HoverTooltip, Modal, ShortcutHelp } from "./AppChrome";

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
  it("uses one tooltip label for the custom tooltip and native title fallback", () => {
    render(<HoverTooltip title="Refresh mail"><button aria-label="Refresh mail">Refresh</button></HoverTooltip>);

    expect(screen.getByRole("button", { name: "Refresh mail" })).toBeInTheDocument();
    expect(screen.getByRole("tooltip")).toHaveTextContent("Refresh mail");
    expect(screen.getByTitle("Refresh mail")).toHaveClass("tooltip-anchor");
  });

  it("opens filters, reports the selected filter, and closes on Escape", () => {
    const onToggleFilter = vi.fn();
    render(<FiltersButton activeFilters={new Set(["starred"])} onToggleFilter={onToggleFilter} />);

    expect(screen.getByRole("button", { name: /Filters/ })).toHaveClass("btn");
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

  it("finds a command by its extra search terms", () => {
    render(<CommandPalette context={context} execute={vi.fn()} onClose={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "Filter Commands" });

    for (const query of ["new task", "create task", "task from email"]) {
      fireEvent.change(input, { target: { value: query } });
      expect(screen.getByRole("button", { name: /Add Task From Conversation/ })).toBeInTheDocument();
    }
    fireEvent.change(input, { target: { value: "new widget" } });
    expect(screen.queryByRole("button", { name: /Add Task From Conversation/ })).not.toBeInTheDocument();
  });

  it("shows readable shortcut keys in the command palette", () => {
    render(<CommandPalette context={context} execute={vi.fn()} onClose={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "Filter Commands" });

    fireEvent.change(input, { target: { value: "command palette" } });
    const palette = screen.getByRole("button", { name: /Command Palette/ });
    expect(within(palette).getByText("⌘/Ctrl + k").tagName).toBe("KBD");
    expect(palette).not.toHaveTextContent("Mod");

    fireEvent.change(input, { target: { value: "go to inbox" } });
    const inbox = screen.getByRole("button", { name: /Go to Inbox/ });
    expect(within(inbox).getByText("g").tagName).toBe("KBD");
    expect(within(inbox).getByText("then").tagName).toBe("SMALL");
    expect(within(inbox).getByText("i").tagName).toBe("KBD");

    fireEvent.change(input, { target: { value: "font size" } });
    const increase = screen.getByRole("button", { name: /Increase Font Size/ });
    expect(within(increase).getByText("⌘/Ctrl + =")).toBeInTheDocument();
    expect(within(increase).getByText("⌘/Ctrl + +")).toBeInTheDocument();
    expect(within(screen.getByRole("button", { name: /Decrease Font Size/ })).getByText("⌘/Ctrl + -")).toBeInTheDocument();
  });

  it("shows a shortcut that repeats a key as two distinct steps", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      render(<CommandPalette context={context} execute={vi.fn()} onClose={vi.fn()} />);
      fireEvent.change(screen.getByRole("textbox", { name: "Filter Commands" }), { target: { value: "go to goals" } });
      const goals = screen.getByRole("button", { name: /Go to Goals/ });
      expect(within(goals).getAllByText("g").map((key) => key.tagName)).toEqual(["KBD", "KBD"]);
      expect(within(goals).getByText("then").tagName).toBe("SMALL");
      expect(consoleError.mock.calls.filter((args) => String(args[0]).includes("same key"))).toEqual([]);
    } finally {
      consoleError.mockRestore();
    }
  });

  it("renders shortcut help and reusable action buttons", () => {
    const action = vi.fn();
    const close = vi.fn();
    const view = render(<ActionButton label="Refresh" shortcut="r" onClick={action}>↻</ActionButton>);

    fireEvent.click(screen.getByRole("button", { name: "Refresh (r)" }));
    expect(action).toHaveBeenCalledTimes(1);
    view.rerender(<ShortcutHelp onClose={close} />);
    const help = screen.getByRole("dialog", { name: "Keyboard Shortcuts" });
    expect(within(help).getByText("Use ThreeStrands without leaving the keyboard.")).toBeInTheDocument();
    const paletteRow = within(help).getByText("Command Palette", { selector: "dt" }).parentElement!;
    expect(within(paletteRow).getByText("⌘/Ctrl + k").tagName).toBe("KBD");

    fireEvent.keyDown(window, { key: "Escape" });
    expect(close).toHaveBeenCalledTimes(1);
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
