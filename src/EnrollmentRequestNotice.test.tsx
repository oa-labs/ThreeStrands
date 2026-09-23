import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./replicatedSync");
const listenMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

import * as sync from "./replicatedSync";
import { EnrollmentRequestNotice } from "./EnrollmentRequestNotice";

const request = (requestId: string): sync.IncomingEnrollmentRequest => ({ requestId, fingerprint: "AAAA-BBBB-CCCC-DDDD", createdAt: "2026-09-22T10:00:00Z" });

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(sync.replicatedSyncEnabled).mockResolvedValue(true);
});
afterEach(cleanup);

describe("EnrollmentRequestNotice", () => {
  it("prompts to review a waiting device and opens Settings", async () => {
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request("req-1")]);
    const onReview = vi.fn();
    render(<EnrollmentRequestNotice suppressed={false} onReview={onReview} />);

    expect(await screen.findByText("A new device is asking to join Replicated Sync.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(onReview).toHaveBeenCalledTimes(1);
  });

  it("counts several waiting devices", async () => {
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request("req-1"), request("req-2")]);
    render(<EnrollmentRequestNotice suppressed={false} onReview={vi.fn()} />);
    expect(await screen.findByText("2 devices are asking to join Replicated Sync.")).toBeInTheDocument();
  });

  it("stays hidden while the Replicated Sync section is already open", async () => {
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request("req-1")]);
    render(<EnrollmentRequestNotice suppressed onReview={vi.fn()} />);
    await waitFor(() => expect(sync.replicatedSyncPendingRequests).toHaveBeenCalled());
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("dismisses the current requests only, showing again when a new one arrives", async () => {
    let onStatus: () => void = () => {};
    listenMock.mockImplementation(async (_event: string, handler: () => void) => { onStatus = handler; return () => {}; });
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request("req-1")]);
    try {
      render(<EnrollmentRequestNotice suppressed={false} onReview={vi.fn()} />);
      fireEvent.click(await screen.findByRole("button", { name: "Dismiss on this device" }));
      expect(screen.queryByRole("status")).not.toBeInTheDocument();
      await waitFor(() => expect(listenMock).toHaveBeenCalledWith("replicated-sync-status", expect.any(Function)));

      // The same request on the next cycle stays dismissed.
      await act(async () => { onStatus(); });
      expect(screen.queryByRole("status")).not.toBeInTheDocument();

      vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request("req-1"), request("req-2")]);
      await act(async () => { onStatus(); });
      expect(await screen.findByText("A new device is asking to join Replicated Sync.")).toBeInTheDocument();
    } finally {
      Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    }
  });

  it("shows a short no-action status when another device resolves a request", async () => {
    let onStatus: () => void = () => {};
    listenMock.mockImplementation(async (_event: string, handler: () => void) => { onStatus = handler; return () => {}; });
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([request("req-1")]);
    try {
      render(<EnrollmentRequestNotice suppressed={false} onReview={vi.fn()} />);
      expect(await screen.findByText("A new device is asking to join Replicated Sync.")).toBeInTheDocument();
      await waitFor(() => expect(listenMock).toHaveBeenCalledWith("replicated-sync-status", expect.any(Function)));
      vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue([]);
      await act(async () => { onStatus(); });
      expect(await screen.findByText("A device request was resolved. No action is needed here.")).toBeInTheDocument();
    } finally {
      Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    }
  });

  it("treats a malformed native response as no requests", async () => {
    vi.mocked(sync.replicatedSyncPendingRequests).mockResolvedValue({} as unknown as sync.IncomingEnrollmentRequest[]);
    render(<EnrollmentRequestNotice suppressed={false} onReview={vi.fn()} />);
    await waitFor(() => expect(sync.replicatedSyncPendingRequests).toHaveBeenCalled());
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("does not ask for requests when replicated sync is off", async () => {
    vi.mocked(sync.replicatedSyncEnabled).mockResolvedValue(false);
    render(<EnrollmentRequestNotice suppressed={false} onReview={vi.fn()} />);
    await waitFor(() => expect(sync.replicatedSyncEnabled).toHaveBeenCalled());
    expect(sync.replicatedSyncPendingRequests).not.toHaveBeenCalled();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
