import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskEditorDialog } from "./TaskEditorDialog";

describe("TaskEditorDialog", () => {
  afterEach(cleanup);

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
    fireEvent.change(screen.getByLabelText("Due date"), { target: { value: "2026-09-25" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({
      title: "Set up the client website",
      dueKind: "date",
      dueValue: "2026-09-25",
    })));
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
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save task");
    expect(title).toHaveValue("Keep this value");
  });
});
