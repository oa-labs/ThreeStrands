import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { DiagnosticsSettings, type SyncDiagnosticsActions } from "./SettingsPanel";
import type { SyncStatus } from "./domain";

afterEach(cleanup);

const healthy: SyncStatus = {
  state: "idle",
  lastSuccessfulSync: "2026-01-02T03:04:05Z",
  cursor: null,
  pendingMutations: 0,
  failedMutations: [],
  quarantinedMessages: [],
  error: null,
};

const rejected: SyncStatus = {
  ...healthy,
  state: "error",
  cursor: "cursor",
  failedMutations: [{
    id: "mutation-1",
    kind: "label",
    threadId: "thread-1",
    attempts: 1,
    error: "Gmail permanently rejected the request: invalid label",
    createdAt: "2026-01-02T03:04:05Z",
  }],
  error: "Gmail permanently rejected the request: invalid label",
};

function actions(): SyncDiagnosticsActions {
  return {
    retryFailed: vi.fn().mockResolvedValue(undefined),
    dismissProblems: vi.fn().mockResolvedValue(undefined),
    dismissRecovery: vi.fn(),
  };
}

describe("DiagnosticsSettings", () => {
  it("keeps crash-report controls in the diagnostics section", () => {
    render(<DiagnosticsSettings status={null} accountCount={1} />);

    expect(screen.getByRole("heading", { name: "Crash Reports" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Share Sanitized Crash Reports" })).toBeInTheDocument();
    // The policy is described in plain language, not as a repository path.
    expect(screen.getByText(/nothing from your mail is included/)).toBeInTheDocument();
    expect(screen.queryByText(/docs\//)).not.toBeInTheDocument();
  });

  it("reports a healthy sync without any issue cards", () => {
    render(<DiagnosticsSettings status={{ ...healthy, pendingMutations: 2 }} accountCount={1} />);

    expect(screen.getByText("Sync is healthy")).toBeInTheDocument();
    expect(screen.getByText(/2 changes waiting to sync/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
  });

  it("shows permanently rejected mailbox operations with their reason once", () => {
    render(<DiagnosticsSettings status={rejected} accountCount={1} />);

    expect(screen.getByText("1 item to review")).toBeInTheDocument();
    const issue = screen.getByRole("group", { name: "1 change couldn’t be applied in Gmail" });
    expect(within(issue).getByText("label")).toBeInTheDocument();
    expect(within(issue).getByText(/1 attempt/)).toBeInTheDocument();
    // The native status repeats the newest failed mutation's error as the
    // sync error; that echo must not appear as a separate sync failure.
    expect(screen.queryByRole("group", { name: "Last sync attempt failed" })).not.toBeInTheDocument();
  });

  it("retries or dismisses failed operations through the supplied actions", async () => {
    const handlers = actions();
    render(<DiagnosticsSettings status={rejected} accountCount={1} actions={handlers} />);
    const issue = screen.getByRole("group", { name: "1 change couldn’t be applied in Gmail" });

    fireEvent.click(within(issue).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(handlers.retryFailed).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(within(issue).getByRole("button", { name: "Dismiss" })).toBeEnabled());

    fireEvent.click(within(issue).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(handlers.dismissProblems).toHaveBeenCalledTimes(1));
  });

  it("shows a failed action's error instead of swallowing it", async () => {
    const handlers = actions();
    vi.mocked(handlers.dismissProblems).mockRejectedValue(new Error("database is locked"));
    render(<DiagnosticsSettings status={rejected} accountCount={1} actions={handlers} />);

    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("database is locked");
  });

  it("keeps a sync error that is not a failed operation's echo", () => {
    render(<DiagnosticsSettings
      status={{
        ...rejected,
        error: "work@example.com: Gmail permanently rejected the request: invalid label\nhome@example.com: network unreachable",
      }}
      accountCount={2}
    />);

    const issue = screen.getByRole("group", { name: "Last sync attempt failed" });
    expect(within(issue).getByText("home@example.com: network unreachable")).toBeInTheDocument();
    expect(within(issue).queryByText(/invalid label/)).not.toBeInTheDocument();
    expect(screen.getByText("2 items to review")).toBeInTheDocument();
  });

  it("shows quarantined message identifiers and normalization errors with a dismiss action", async () => {
    const handlers = actions();
    const status: SyncStatus = {
      ...healthy,
      cursor: "advanced-cursor",
      quarantinedMessages: [{
        messageId: "bad-message",
        threadId: "provider-thread",
        error: "Invalid Gmail base64url body",
        createdAt: "2026-01-02T03:04:05Z",
      }],
    };

    render(<DiagnosticsSettings status={status} accountCount={1} actions={handlers} />);

    const issue = screen.getByRole("group", { name: "1 message couldn’t be read" });
    expect(within(issue).getByText("Message bad-message")).toBeInTheDocument();
    expect(within(issue).getByText((_, element) => (
      element?.tagName === "LI"
      && element.textContent?.includes("Invalid Gmail base64url body")
      && element.textContent.includes("Thread provider-thread")
    ))).toBeInTheDocument();
    fireEvent.click(within(issue).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(handlers.dismissProblems).toHaveBeenCalledTimes(1));
  });

  it("lets a database recovery notice be dismissed", () => {
    const handlers = actions();
    render(<DiagnosticsSettings
      status={healthy}
      recovery={{ kind: "freshDatabase", corruptPath: null }}
      accountCount={1}
      actions={handlers}
    />);

    const issue = screen.getByRole("group", { name: "Mail cache was recovered" });
    expect(within(issue).getByText(/rebuilt from scratch/)).toBeInTheDocument();
    fireEvent.click(within(issue).getByRole("button", { name: "Dismiss" }));
    expect(handlers.dismissRecovery).toHaveBeenCalledTimes(1);
  });

  it("shows the history cursor while a single account is connected", () => {
    render(<DiagnosticsSettings status={healthy} accountCount={1} />);

    expect(screen.getByText("Technical details")).toBeInTheDocument();
    expect(screen.getByText("History cursor")).toBeInTheDocument();
    expect(screen.getByText("Not initialized")).toBeInTheDocument();
  });

  it("hides the history cursor once several accounts are connected", () => {
    // The merged status deliberately carries no cursor across accounts —
    // one position cannot describe several mailboxes — so showing
    // "Not initialized" here would misreport every healthy account.
    render(<DiagnosticsSettings status={healthy} accountCount={3} />);

    expect(screen.queryByText("History cursor")).not.toBeInTheDocument();
    expect(screen.queryByText("Not initialized")).not.toBeInTheDocument();
    expect(screen.getByText("Last successful sync")).toBeInTheDocument();
  });
});
