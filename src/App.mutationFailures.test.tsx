import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { DiagnosticsSettings } from "./App";
import type { SyncStatus } from "./domain";

afterEach(cleanup);

describe("mutation failure diagnostics", () => {
  it("keeps crash-report controls in the diagnostics section", () => {
    render(<DiagnosticsSettings status={null} accountCount={1} />);

    expect(screen.getByRole("heading", { name: "Crash Reports" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Share Sanitized Crash Reports" })).toBeInTheDocument();
  });

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

    render(<DiagnosticsSettings status={status} accountCount={1} />);

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

    render(<DiagnosticsSettings status={status} accountCount={1} />);

    expect(screen.getByText("Quarantined messages")).toBeInTheDocument();
    expect(screen.getByText("Message bad-message")).toBeInTheDocument();
    expect(screen.getByText((_, element) => (
      element?.tagName === "LI"
      && element.textContent?.includes("Invalid Gmail base64url body")
      && element.textContent.includes("Thread provider-thread")
    ))).toBeInTheDocument();
  });

  it("shows the history cursor while a single account is connected", () => {
    const status: SyncStatus = {
      state: "idle",
      lastSuccessfulSync: "2026-01-02T03:04:05Z",
      cursor: null,
      pendingMutations: 0,
      failedMutations: [],
      quarantinedMessages: [],
      error: null,
    };

    render(<DiagnosticsSettings status={status} accountCount={1} />);

    expect(screen.getByText("History cursor")).toBeInTheDocument();
    expect(screen.getByText("Not initialized")).toBeInTheDocument();
  });

  it("hides the history cursor once several accounts are connected", () => {
    // The merged status deliberately carries no cursor across accounts —
    // one position cannot describe several mailboxes — so showing
    // "Not initialized" here would misreport every healthy account.
    const status: SyncStatus = {
      state: "idle",
      lastSuccessfulSync: "2026-01-02T03:04:05Z",
      cursor: null,
      pendingMutations: 0,
      failedMutations: [],
      quarantinedMessages: [],
      error: null,
    };

    render(<DiagnosticsSettings status={status} accountCount={3} />);

    expect(screen.queryByText("History cursor")).not.toBeInTheDocument();
    expect(screen.queryByText("Not initialized")).not.toBeInTheDocument();
    expect(screen.getByText("Last successful sync")).toBeInTheDocument();
  });
});
