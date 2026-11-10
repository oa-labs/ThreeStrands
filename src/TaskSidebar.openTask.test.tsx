import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { TaskSidebar, type TaskWorkspaceHandle } from "./TaskSidebar";
import { mailClient } from "./data/client";
import type { ThreadTask } from "./domain";
afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });
it("opens an exact linked task after loading, including a completed task hidden by the current view", async () => {
  const row = (id: string, status: ThreadTask["status"]): ThreadTask => ({ id, accountId: "you@example.com", threadId: null, subjectSnapshot: null, title: id, kind: "action", dueKind: "none", status, createdAt: "2026-11-01T00:00:00Z", updatedAt: "2026-11-01T00:00:00Z", completedAt: status === "completed" ? "2026-11-01T00:00:00Z" : null });
  vi.spyOn(mailClient, "listTasks").mockResolvedValue([row("First task", "open"), row("Linked task", "completed")]);
  vi.spyOn(mailClient, "listGoals").mockResolvedValue([]);
  const ref = createRef<TaskWorkspaceHandle>();
  const { container } = render(<TaskSidebar ref={ref} accountId="you@example.com" onOpenThread={vi.fn()} />);
  act(() => ref.current!.openTask("Linked task"));
  await waitFor(() => expect(container.querySelector(".task-card.selected strong")).toHaveTextContent("Linked task"));
  expect(within(await screen.findByRole("complementary", { name: "Task context" })).getByRole("textbox", { name: "Title" })).toHaveValue("Linked task");
});
