import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Diagnostics } from "./App";
import type { SyncStatus } from "./domain";

afterEach(cleanup);

describe("mutation failure diagnostics", () => {
  it("shows permanently rejected mailbox operations with their reason", () => {
    const status: SyncStatus = {
      state: "error",
      lastSuccessfulSync: null,
      cursor: "cursor",
      pendingMutations: 0,
      failedMutations: [{
        id: "mutation-1",
        kind: "label",
        threadId: "thread-1",
        attempts: 1,
        error: "Gmail permanently rejected the request: invalid label",
        createdAt: "2026-01-02T03:04:05Z",
      }],
      quarantinedMessages: [],
      error: "Gmail permanently rejected the request: invalid label",
    };

    render(<Diagnostics status={status} onClose={vi.fn()} />);

    expect(screen.getByText("Permanently failed operations")).toBeInTheDocument();
    expect(screen.getByText("label")).toBeInTheDocument();
    expect(screen.getAllByText(/invalid label/)).toHaveLength(2);
    expect(screen.getByText(/1 attempt/)).toBeInTheDocument();
  });

  it("shows quarantined message identifiers and normalization errors", () => {
    const status: SyncStatus = {
      state: "idle",
      lastSuccessfulSync: "2026-01-02T03:04:05Z",
      cursor: "advanced-cursor",
      pendingMutations: 0,
      failedMutations: [],
      quarantinedMessages: [{
        messageId: "bad-message",
        threadId: "provider-thread",
        error: "Invalid Gmail base64url body",
        createdAt: "2026-01-02T03:04:05Z",
      }],
      error: null,
    };

    render(<Diagnostics status={status} onClose={vi.fn()} />);

    expect(screen.getByText("Quarantined messages")).toBeInTheDocument();
    expect(screen.getByText("Message bad-message")).toBeInTheDocument();
    expect(screen.getByText((_, element) => (
      element?.tagName === "LI"
      && element.textContent?.includes("Invalid Gmail base64url body")
      && element.textContent.includes("Thread provider-thread")
    ))).toBeInTheDocument();
  });
});
