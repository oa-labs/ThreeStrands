import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ComponentProps } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LabelManager } from "./LabelManager";
import type { Label } from "./domain";
import { labelCapabilities } from "./labels";

afterEach(cleanup);

const labels: Label[] = [
  { id: "lf:Clients", name: "Clients", kind: "user" },
  { id: "lf:Projects", name: "Projects", kind: "user" },
];

function renderManager(labelModel: Parameters<typeof labelCapabilities>[0]) {
  return render(
    <LabelManager
      labels={labels}
      accountId="imap@example.com"
      capabilities={labelCapabilities(labelModel)}
      checkedLabelIds={new Set()}
      onClose={() => {}}
      onCreate={vi.fn(async () => labels[0]!)}
      onDelete={vi.fn(async () => {})}
      onRename={vi.fn(async () => {})}
      onToggle={vi.fn()}
    />,
  );
}

describe("LabelManager capability gating", () => {
  it("offers create (via the picker), delete and rename for Gmail labels", () => {
    renderManager("gmailLabels");
    expect(screen.getByRole("combobox", { name: "Find or Create a Label" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Rename Clients" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Clients" })).toBeInTheDocument();
  });

  it("hides rename but keeps create/delete for IMAP label folders", () => {
    renderManager("imapLabelFolders");
    // Create still available via the picker, delete still available…
    expect(screen.getByRole("combobox", { name: "Find or Create a Label" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete Clients" })).toBeInTheDocument();
    // …but rename can only fail and roll back, so it is not offered.
    expect(screen.queryByRole("button", { name: "Rename Clients" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Rename Projects" })).not.toBeInTheDocument();
  });

  it("hides all label controls and explains why for an IMAP account with no label storage", () => {
    renderManager("imapNoUserLabels");
    const dialog = screen.getByRole("dialog", { name: "Manage Labels" });
    expect(within(dialog).queryByRole("combobox", { name: "Find or Create a Label" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Delete Clients" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Rename Clients" })).not.toBeInTheDocument();
    // A short, provider-neutral explanation is shown instead.
    expect(within(dialog).getByText(/does not support labels/i)).toBeInTheDocument();
  });

  it("hides all label controls for an IMAP keyword account too", () => {
    renderManager("imapKeywords");
    const dialog = screen.getByRole("dialog", { name: "Manage Labels" });
    expect(within(dialog).queryByRole("combobox", { name: "Find or Create a Label" })).not.toBeInTheDocument();
    expect(within(dialog).getByText(/does not support labels/i)).toBeInTheDocument();
  });
});

describe("LabelManager picker", () => {
  const work: Label = { id: "work", name: "Work", kind: "user" };
  const keyboard: Label = { id: "keyboard", name: "Keyboard navigation", kind: "user" };

  afterEach(() => localStorage.clear());

  function renderPicker(overrides: Partial<ComponentProps<typeof LabelManager>> = {}) {
    const props = {
      labels: [work, keyboard],
      accountId: "demo@example.com",
      capabilities: labelCapabilities("gmailLabels"),
      checkedLabelIds: new Set<string>(),
      onClose: vi.fn(),
      onCreate: vi.fn(async (name: string): Promise<Label> => ({ id: `new-${name}`, name, kind: "user" })),
      onDelete: vi.fn(async () => {}),
      onRename: vi.fn(async () => {}),
      onToggle: vi.fn(),
      ...overrides,
    };
    render(<LabelManager {...props} />);
    return props;
  }

  it("orders labels alphabetically and supports search plus keyboard navigation", async () => {
    const { onToggle, onClose } = renderPicker();
    const dialog = screen.getByRole("dialog", { name: "Manage Labels" });
    const input = within(dialog).getByRole("combobox", { name: "Find or Create a Label" });
    expect(input).toHaveFocus();
    expect(within(dialog).getAllByRole("option").map((option) => option.textContent))
      .toEqual(["Keyboard navigation", "Work"]);
    expect(within(dialog).getAllByRole("option")[0]).toHaveClass("highlighted");

    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(within(dialog).getAllByRole("option")[1]).toHaveClass("highlighted");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(within(dialog).getAllByRole("option")[0]).toHaveClass("highlighted");

    fireEvent.change(input, { target: { value: "key" } });
    expect(within(dialog).getAllByRole("option").map((option) => option.textContent))
      .toEqual(["Keyboard navigation", 'Create label "key"']);

    fireEvent.keyDown(input, { key: "Enter" });
    expect(onToggle).toHaveBeenCalledWith(keyboard, true);
    expect(onClose).toHaveBeenCalled();
  });

  it("creates and applies a new label from the search box", async () => {
    const { onCreate, onToggle, onClose } = renderPicker();
    const input = screen.getByRole("combobox", { name: "Find or Create a Label" });
    fireEvent.change(input, { target: { value: "Project X" } });
    const createRow = screen.getByRole("option", { name: 'Create label "Project X"' });
    expect(createRow).toHaveClass("highlighted");

    await act(async () => {
      fireEvent.keyDown(input, { key: "Enter" });
    });
    expect(onCreate).toHaveBeenCalledWith("Project X");
    const created = { id: "new-Project X", name: "Project X", kind: "user" };
    expect(onToggle).toHaveBeenCalledWith(created, true);
    expect(onClose).toHaveBeenCalled();
  });

  it("marks an applied label as added and removes it when picked again", () => {
    const { onToggle } = renderPicker({ checkedLabelIds: new Set([work.id]) });
    fireEvent.click(screen.getByRole("option", { name: "Work, added" }));
    expect(onToggle).toHaveBeenCalledWith(work, false);
  });
});
