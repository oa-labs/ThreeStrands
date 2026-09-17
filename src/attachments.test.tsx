import { act, cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";

describe("message attachments", () => {
  beforeEach(async () => {
    for (const threadId of ["welcome", "roadmap", "privacy"]) {
      await mailClient.mutateThread({ kind: "archive", threadId, value: false });
      await mailClient.mutateThread({ kind: "trash", threadId, value: false });
    }
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("shows a list paperclip only when a thread has attachments", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    const welcomeRow = screen.getAllByText("Welcome to ThreeStrands")
      .find((element) => element.classList.contains("thread-subject"))
      ?.closest(".thread-row");
    const roadmapRow = screen.getByText("Phase 1: read and triage").closest(".thread-row");
    expect(welcomeRow).toBeTruthy();
    expect(roadmapRow).not.toBeNull();
    expect(within(welcomeRow as HTMLElement).getByLabelText("Has attachments")).toBeVisible();
    expect(within(roadmapRow as HTMLElement).queryByLabelText("Has attachments")).toBeNull();
  });

  it("shows reader badges and routes view and download actions", async () => {
    const open = vi.spyOn(mailClient, "openAttachment").mockResolvedValue();
    const save = vi.spyOn(mailClient, "saveAttachment").mockResolvedValue();
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    const view = await screen.findByRole("button", { name: "View threestrands-shortcuts.txt" });
    const download = screen.getByRole("button", { name: "Download threestrands-shortcuts.txt" });
    expect(view).toBeVisible();

    await act(async () => {
      view.click();
      download.click();
    });
    expect(open).toHaveBeenCalledWith("welcome-message", "demo-guide");
    expect(save).toHaveBeenCalledWith("welcome-message", "demo-guide");
  });
});
