import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CommandContext } from "./commands";
import { ActionButton, CommandPalette, FiltersButton, ShortcutHelp } from "./AppChrome";

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

    const input = screen.getByRole("textbox", { name: "Filter commands" });
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
    render(
      <>
        <ActionButton label="Refresh" shortcut="r" onClick={action}>↻</ActionButton>
        <ShortcutHelp onClose={close} />
      </>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Refresh (r)" }));
    expect(action).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeInTheDocument();
    expect(screen.getByText("Use ThreeStrands without leaving the keyboard.")).toBeInTheDocument();
  });
});
