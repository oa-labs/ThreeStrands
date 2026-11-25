import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DoneByEmail } from "./DoneByEmail";
import { mailClient } from "./data/client";
import type { ThreadTask } from "./domain";

const finished: ThreadTask = {
  id: "t", accountId: "me@example.com", threadId: null, subjectSnapshot: null, title: "Send the contract", kind: "action", dueKind: "none",
  status: "completed", completionSource: "user", createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", completedAt: "2026-09-20T10:00:00Z",
  completedByEmail: { accountId: "me@example.com", subject: "The contract", sentAt: 1, messageId: "m-1" },
};
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("DoneByEmail", () => {
  it("names the email and opens its conversation when the message is on this device", async () => {
    const lookup = vi.spyOn(mailClient, "threadForMessage").mockResolvedValue("thread-1");
    const onOpen = vi.fn();
    render(<DoneByEmail task={finished} onOpen={onOpen} />);
    expect(screen.getByText("Done by email")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "The contract" }));
    await waitFor(() => expect(onOpen).toHaveBeenCalledWith("thread-1", "m-1"));
    expect(lookup).toHaveBeenCalledWith("me@example.com", "m-1");
  });

  it.each([["has not synced here", "m-1", null], ["has no key yet", null, "unused"]])("says so when the message %s", async (_name, messageId, found) => {
    const lookup = vi.spyOn(mailClient, "threadForMessage").mockResolvedValue(found);
    const onOpen = vi.fn();
    render(<DoneByEmail task={{ ...finished, completedByEmail: { ...finished.completedByEmail!, messageId } }} onOpen={onOpen} />);
    fireEvent.click(screen.getByRole("button", { name: "The contract" }));
    expect(await screen.findByRole("status")).toHaveTextContent("That message isn't on this device yet.");
    expect(onOpen).not.toHaveBeenCalled();
    if (!messageId) expect(lookup).not.toHaveBeenCalled();
  });

  it("reports a failed lookup instead of claiming the message is missing", async () => {
    vi.spyOn(mailClient, "threadForMessage").mockRejectedValue(new Error("Database is locked"));
    render(<DoneByEmail task={finished} onOpen={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "The contract" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Database is locked");
  });

  it("shows nothing without a record, or once the task is reopened elsewhere", () => {
    const { container, rerender } = render(<DoneByEmail task={{ ...finished, completedByEmail: null }} onOpen={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
    // Another device can reopen a task without clearing the record.
    rerender(<DoneByEmail task={{ ...finished, status: "open" }} onOpen={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("falls back to a placeholder for an empty subject", () => {
    render(<DoneByEmail task={{ ...finished, completedByEmail: { ...finished.completedByEmail!, subject: " " } }} onOpen={vi.fn()} />);
    expect(screen.getByRole("button", { name: "(no subject)" })).toBeInTheDocument();
  });
});
