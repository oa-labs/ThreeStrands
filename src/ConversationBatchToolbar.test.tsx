import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { createRef, type ComponentProps } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConversationBatchToolbar } from "./ConversationBatchToolbar";

afterEach(cleanup);

function renderToolbar(overrides: Partial<ComponentProps<typeof ConversationBatchToolbar>> = {}) {
  const props = {
    inTrash: false,
    selectedCount: 1,
    allVisibleChecked: false,
    allStarred: false,
    selectAllRef: createRef<HTMLInputElement>(),
    onSelectAll: vi.fn(),
    onClearSelection: vi.fn(),
    onRun: vi.fn(),
    onLabels: vi.fn(),
    ...overrides,
  };
  const view = render(<ConversationBatchToolbar {...props} />);
  return { ...props, ...view, toolbar: screen.getByRole("toolbar", { name: "Batch actions" }) };
}

describe("ConversationBatchToolbar", () => {
  it("shows one state-aware star action and hover help for every batch action", () => {
    const { toolbar, onRun, rerender, ...props } = renderToolbar();
    expect(within(toolbar).getByText("1 selected")).toBeInTheDocument();
    expect(within(toolbar).getAllByRole("button", { name: /star/i })).toHaveLength(1);

    for (const label of ["Archive", "Trash", "Mark spam", "Mark read", "Mark unread", "Star", "Labels", "Clear selection"]) {
      expect(within(toolbar).getByRole("tooltip", { name: label })).toBeInTheDocument();
    }

    fireEvent.click(within(toolbar).getByRole("button", { name: "Star" }));
    expect(onRun).toHaveBeenCalledWith("Star", { kind: "star", value: true });

    rerender(<ConversationBatchToolbar {...props} onRun={onRun} allStarred />);
    const starred = screen.getByRole("toolbar", { name: "Batch actions" });
    expect(within(starred).getAllByRole("button", { name: /star/i })).toHaveLength(1);
    fireEvent.click(within(starred).getByRole("button", { name: "Unstar" }));
    expect(onRun).toHaveBeenLastCalledWith("Unstar", { kind: "star", value: false });
  });

  it("offers Restore in place of Archive, Trash, and Mark Spam in the trash", () => {
    const { toolbar, onRun } = renderToolbar({ inTrash: true });
    for (const name of ["Archive", "Trash", "Mark Spam"]) {
      expect(within(toolbar).queryByRole("button", { name })).not.toBeInTheDocument();
    }
    fireEvent.click(within(toolbar).getByRole("button", { name: "Restore" }));
    expect(onRun).toHaveBeenCalledWith("Restore", { kind: "trash", value: false });
  });

  it("selects all, clears, and opens labels for the checked conversations", () => {
    const { toolbar, onSelectAll, onClearSelection, onLabels, selectAllRef } = renderToolbar();
    const selectAll = within(toolbar).getByRole("checkbox", { name: "Select All Conversations" });
    expect(selectAllRef.current).toBe(selectAll);
    fireEvent.click(selectAll);
    expect(onSelectAll).toHaveBeenCalledTimes(1);
    fireEvent.click(within(toolbar).getByRole("button", { name: "Labels" }));
    expect(onLabels).toHaveBeenCalledTimes(1);
    fireEvent.click(within(toolbar).getByRole("button", { name: "Clear Selection" }));
    expect(onClearSelection).toHaveBeenCalledTimes(1);
  });

  it("clears the selection when the checked select-all box is unchecked", () => {
    const { toolbar, onSelectAll, onClearSelection } = renderToolbar({ allVisibleChecked: true });
    fireEvent.click(within(toolbar).getByRole("checkbox", { name: "Select All Conversations" }));
    expect(onClearSelection).toHaveBeenCalledTimes(1);
    expect(onSelectAll).not.toHaveBeenCalled();
  });
});
