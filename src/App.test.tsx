import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App, NOTICE_TIMEOUT_MS } from "./App";
import { mailClient } from "./data/client";

const demoThreadIds = ["welcome", "roadmap", "privacy"];

async function archiveSelected() {
  const button = await screen.findByRole("button", { name: "Archive (e)" });
  await act(async () => {
    button.click();
  });
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("archive notice", () => {
  beforeEach(async () => {
    for (const threadId of demoThreadIds) {
      await mailClient.mutateThread({ kind: "archive", threadId, value: false });
    }
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("dismisses itself after the notice timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    expect(await screen.findByRole("status")).toHaveTextContent("Conversation archived");

    await advance(NOTICE_TIMEOUT_MS - 500);
    expect(screen.getByRole("status")).toBeInTheDocument();

    await advance(500);
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
  });

  it("can still be dismissed manually before the timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    const dismiss = await screen.findByRole("button", { name: "Dismiss" });
    await act(async () => {
      dismiss.click();
    });

    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("keeps a replacement notice on screen for its own full timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    await advance(NOTICE_TIMEOUT_MS - 1000);
    await archiveSelected();

    await advance(1500);
    expect(screen.getByRole("status")).toHaveTextContent("Conversation archived");

    await advance(NOTICE_TIMEOUT_MS);
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
  });

  it("clears the pending dismiss timer when the app unmounts", async () => {
    const setTimeout = vi.spyOn(window, "setTimeout");
    const clearTimeout = vi.spyOn(window, "clearTimeout");
    const { unmount } = render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    await screen.findByRole("status");
    const dismissTimer = setTimeout.mock.results
      .filter((_, index) => setTimeout.mock.calls[index][1] === NOTICE_TIMEOUT_MS)
      .map((result) => result.value)
      .at(-1);
    expect(dismissTimer).toBeDefined();

    unmount();
    expect(clearTimeout).toHaveBeenCalledWith(dismissTimer);
    setTimeout.mockRestore();
    clearTimeout.mockRestore();
  });
});
