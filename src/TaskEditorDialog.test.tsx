import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Goal } from "./domain";
import { TaskEditorDialog } from "./TaskEditorDialog";
import { expectPrimaryActionLast } from "./test/sharedButtons";

describe("TaskEditorDialog", () => {
  afterEach(cleanup);

  it("places Type and Due side by side in one row", () => {
    render(<TaskEditorDialog initial={{ title: "", kind: "action", dueKind: "none" }} onClose={vi.fn()} onSubmit={vi.fn()} />);
    const row = screen.getByRole("combobox", { name: "Type" }).closest(".modal-form-row");
    expect(row).not.toBeNull();
    expect(row).toContainElement(screen.getByRole("combobox", { name: "Due" }));
  });

  it("labels the free-text field Description", () => {
    render(<TaskEditorDialog initial={{ title: "A", kind: "action", dueKind: "none", notes: "Detail" }} onClose={vi.fn()} onSubmit={vi.fn()} />);
    expect(screen.getByRole("textbox", { name: "Description" })).toHaveValue("Detail");
    expect(screen.queryByRole("textbox", { name: "Notes" })).not.toBeInTheDocument();
  });

  it("offers Add Note only when asked, closing without saving when nothing changed", async () => {
    const { unmount } = render(<TaskEditorDialog initial={{ title: "A", kind: "action", dueKind: "none" }} onClose={vi.fn()} onSubmit={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Add Note" })).not.toBeInTheDocument();
    unmount();
    const onClose = vi.fn(), onSubmit = vi.fn().mockResolvedValue(undefined), onAddNote = vi.fn();
    render(<TaskEditorDialog initial={{ title: "Plan launch", kind: "action", dueKind: "none" }} submitLabel="Save Task" onClose={onClose} onSubmit={onSubmit} onAddNote={onAddNote} />);
    fireEvent.click(screen.getByRole("button", { name: "Add Note" }));
    await waitFor(() => expect(onAddNote).toHaveBeenCalledWith("Plan launch"));
    expect(onClose).toHaveBeenCalled();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("saves unsaved edits before Add Note so none are discarded", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined), onAddNote = vi.fn();
    render(<TaskEditorDialog initial={{ title: "Plan launch", kind: "action", dueKind: "none" }} submitLabel="Save Task" onClose={vi.fn()} onSubmit={onSubmit} onAddNote={onAddNote} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Task" }), { target: { value: "Plan the launch" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Note" }));
    await waitFor(() => expect(onAddNote).toHaveBeenCalledWith("Plan the launch"));
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ title: "Plan the launch" }));
  });

  it("owns text entry and submits reviewed task values", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    render(
      <TaskEditorDialog
        initial={{ title: "Website setup", kind: "action", dueKind: "none" }}
        sourceSubject="Website setup"
        evidence="Please set up the website by Friday."
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    const title = screen.getByRole("textbox", { name: "Task" });
    await waitFor(() => expect(title).toHaveFocus());
    fireEvent.change(title, { target: { value: "Set up the client website" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Due" }), { target: { value: "date" } });
    fireEvent.change(screen.getByLabelText("Due Date"), { target: { value: "2026-09-25" } });
    expectPrimaryActionLast(screen.getByRole("button", { name: "Add Task" }).parentElement!);
    fireEvent.click(screen.getByRole("button", { name: "Add Task" }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({
      title: "Set up the client website",
      dueKind: "date",
      dueValue: "2026-09-25",
    })));
  });

  it("rejects an invalid timezone before submitting a dated task", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    render(
      <TaskEditorDialog
        initial={{ title: "Plan launch", kind: "action", dueKind: "none" }}
        onClose={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.change(screen.getByRole("combobox", { name: "Due" }), { target: { value: "date" } });
    fireEvent.change(screen.getByLabelText("Due Date"), { target: { value: "2026-09-25" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Timezone" }), { target: { value: "America/New Yok" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Task" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Choose a valid timezone");
    expect(onSubmit).not.toHaveBeenCalled();

    fireEvent.change(screen.getByRole("combobox", { name: "Timezone" }), { target: { value: "America/New_York" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Task" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ timeZone: "America/New_York" })));
  });

  it("preserves the entered date when switching between due date and due date-and-time", async () => {
    render(
      <TaskEditorDialog
        initial={{ title: "Plan launch", kind: "action", dueKind: "none" }}
        onClose={vi.fn()}
        onSubmit={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByRole("combobox", { name: "Due" }), { target: { value: "date" } });
    fireEvent.change(screen.getByLabelText("Due Date"), { target: { value: "2026-09-25" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Due" }), { target: { value: "datetime" } });
    expect(screen.getByLabelText("Due Date and Time")).toHaveValue("2026-09-25T09:00");

    fireEvent.change(screen.getByRole("combobox", { name: "Due" }), { target: { value: "date" } });
    expect(screen.getByLabelText("Due Date")).toHaveValue("2026-09-25");
  });

  it("preserves entered values when submission fails", async () => {
    render(
      <TaskEditorDialog
        initial={{ title: "Original", kind: "action", dueKind: "none" }}
        sourceSubject="Website setup"
        onClose={vi.fn()}
        onSubmit={vi.fn().mockRejectedValue(new Error("Could not save task"))}
      />,
    );
    const title = screen.getByRole("textbox", { name: "Task" });
    fireEvent.change(title, { target: { value: "Keep this value" } });
    fireEvent.click(screen.getByRole("button", { name: "Add Task" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save task");
    expect(title).toHaveValue("Keep this value");
  });
  describe("goals", () => {
    const goal = (id: string, overrides: Partial<Goal> = {}): Goal => ({
      id, accountId: "you@example.com", title: id, notes: null, horizon: "quarter", period: "2026-Q4", status: "active",
      parentGoalId: null, createdAt: "2026-10-01T00:00:00Z", updatedAt: "2026-10-01T00:00:00Z", closedAt: null, ...overrides,
    });
    const goals = [goal("Ship IMAP"), goal("Grow", { horizon: "year", period: "2026" }), goal("Closed", { status: "dropped" }), goal("Theirs", { accountId: "other@example.com" })];

    it("pre-fills a suggested goal, marks it for confirmation, and submits the chosen goal", async () => {
      const onSubmit = vi.fn().mockResolvedValue(undefined);
      render(<TaskEditorDialog initial={{ title: "Send proposal", goalId: "Ship IMAP" }} goals={goals} accountId="you@example.com" goalSuggested onClose={vi.fn()} onSubmit={onSubmit} />);

      const select = screen.getByRole("combobox", { name: /Goal/ });
      expect(select).toHaveValue("Ship IMAP");
      expect(screen.getByText("· Suggested")).toBeInTheDocument();
      expect([...select.querySelectorAll("option")].map((option) => option.value)).toEqual(["", "Ship IMAP", "Grow"]);
      fireEvent.change(select, { target: { value: "Grow" } });
      expect(screen.queryByText("· Suggested")).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Add Task" }));
      await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ goalId: "Grow" })));
    });

    it("drops a suggested goal that is closed or unknown and submits no goal", async () => {
      const onSubmit = vi.fn().mockResolvedValue(undefined);
      render(<TaskEditorDialog initial={{ title: "Send proposal", goalId: "invented" }} goals={goals} accountId="you@example.com" goalSuggested onClose={vi.fn()} onSubmit={onSubmit} />);
      expect(screen.getByRole("combobox", { name: /Goal/ })).toHaveValue("");
      expect(screen.queryByText("· Suggested")).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Add Task" }));
      await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ goalId: null })));
    });

    it("leaves goals out of the values until the account's goals are loaded", async () => {
      const onSubmit = vi.fn().mockResolvedValue(undefined);
      render(<TaskEditorDialog initial={{ title: "Send proposal", goalId: "Ship IMAP" }} goals={null} accountId="you@example.com" onClose={vi.fn()} onSubmit={onSubmit} />);
      expect(screen.queryByRole("combobox", { name: /Goal/ })).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Add Task" }));
      await waitFor(() => expect(onSubmit).toHaveBeenCalled());
      expect(onSubmit.mock.calls[0][0]).not.toHaveProperty("goalId");
    });
  });
});
