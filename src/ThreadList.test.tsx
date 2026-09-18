import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Thread } from "./domain";
import { HighlightedSnippet, ThreadRow } from "./ThreadList";

afterEach(cleanup);

const thread: Thread = {
  id: "thread-1",
  providerThreadId: "provider-1",
  subject: "Roadmap update",
  snippet: "Plain &amp; safe",
  participants: ["Ada Lovelace <ada@example.com>"],
  lastMessageAt: "2026-09-18T12:00:00Z",
  lastReceivedAt: "2026-09-18T12:00:00Z",
  unread: true,
  starred: true,
  archived: false,
  trashed: false,
  labels: [],
  accountId: "ada@example.com",
  matchSnippet: "Before \u0001matched\u0002 after",
  summary: null,
  summaryGeneratedAt: null,
  hasAttachments: true,
};

describe("ThreadList", () => {
  it("renders highlighted search snippets without treating them as markup", () => {
    const { rerender } = render(<HighlightedSnippet thread={thread} />);
    expect(screen.getByText("matched")).toBeInTheDocument();
    expect(screen.getByText("matched").tagName).toBe("MARK");

    rerender(<HighlightedSnippet thread={{ ...thread, matchSnippet: null }} />);
    expect(screen.getByText("Plain & safe")).toBeInTheDocument();
  });

  it("separates row selection from the checkbox action", () => {
    const onSelect = vi.fn();
    const onToggleCheck = vi.fn();
    render(
      <ThreadRow
        thread={thread}
        selected
        checked
        showAccount
        accountColor="#123456"
        onSelect={onSelect}
        onToggleCheck={onToggleCheck}
      />,
    );

    const row = screen.getByRole("option", { name: /Roadmap update/ });
    expect(row).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("Selected for batch actions")).toBeInTheDocument();
    expect(screen.getByLabelText("Has attachments")).toBeInTheDocument();

    fireEvent.click(row.querySelector(".row-check")!);
    expect(onToggleCheck).toHaveBeenCalledWith("thread-1");
    expect(onSelect).not.toHaveBeenCalled();

    fireEvent.click(row);
    expect(onSelect).toHaveBeenCalledWith("thread-1");
  });
});
