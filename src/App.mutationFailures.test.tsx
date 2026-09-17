import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Diagnostics } from "./App";
import type { SyncStatus } from "./domain";

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
      error: "Gmail permanently rejected the request: invalid label",
    };

    render(<Diagnostics status={status} onClose={vi.fn()} />);

    expect(screen.getByText("Permanently failed operations")).toBeInTheDocument();
    expect(screen.getByText("label")).toBeInTheDocument();
    expect(screen.getAllByText(/invalid label/)).toHaveLength(2);
    expect(screen.getByText(/1 attempt/)).toBeInTheDocument();
  });
});
