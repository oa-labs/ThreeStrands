import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";

describe("split inbox search shortcuts", () => {
  afterEach(() => {
    cleanup();
    localStorage.removeItem("threestrands.settings.selectedAccountId");
    localStorage.removeItem("threestrands.settings.selectedTabByAccount");
    vi.restoreAllMocks();
  });

  it("keeps the search query while cycling or clicking between mailbox tabs", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    const searchThreads = vi.spyOn(mailClient, "searchThreads");
    const backfillSearchThreads = vi.spyOn(mailClient, "backfillSearchThreads");
    vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([
      {
        id: "work-split",
        name: "Work",
        matchKind: "label",
        matchValue: "work",
        sortOrder: 0,
        createdAt: "2026-03-01T00:00:00Z",
        accountId: "demo@example.com",
      },
    ]);

    render(<App />);
    const splitTab = await screen.findByRole("tab", { name: "Work" });
    fireEvent.click(splitTab);
    await waitFor(() => expect(splitTab).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("button", { name: "Inbox (g then i)" })).toHaveClass("active");

    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search mail" });
    expect(search).toHaveFocus();
    expect(splitTab).toHaveAttribute("aria-selected", "true");
    fireEvent.change(search, { target: { value: "roadmap" } });
    await waitFor(() => expect(searchThreads).toHaveBeenCalledWith(
      expect.objectContaining({ query: "roadmap" }),
      "demo@example.com",
    ));
    fireEvent.click(screen.getByRole("button", { name: "Include archived or trashed mail in search" }));
    await waitFor(() => expect(backfillSearchThreads).toHaveBeenCalledWith("roadmap", "demo@example.com"));

    fireEvent.keyDown(search, { key: "Tab" });
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Inbox/ })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("textbox", { name: "Search mail" })).toHaveValue("roadmap");
    expect(screen.getByRole("textbox", { name: "Search mail" })).toHaveFocus();

    fireEvent.click(splitTab);
    await waitFor(() => expect(splitTab).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("textbox", { name: "Search mail" })).toHaveValue("roadmap");

    fireEvent.keyDown(screen.getByRole("textbox", { name: "Search mail" }), { key: "Tab", shiftKey: true });
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Inbox/ })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("textbox", { name: "Search mail" })).toHaveValue("roadmap");
  });

  it("shows when the Gmail backfill is still running", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    let finishBackfill!: () => void;
    const backfillSearchThreads = vi.spyOn(mailClient, "backfillSearchThreads").mockImplementation(
      () => new Promise<void>((resolve) => { finishBackfill = resolve; }),
    );

    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search mail" });
    fireEvent.change(search, { target: { value: "126" } });
    fireEvent.click(await screen.findByRole("button", { name: "Include archived or trashed mail in search" }));

    await waitFor(() => expect(backfillSearchThreads).toHaveBeenCalledWith("126", "demo@example.com"));
    expect(await screen.findByText("Searching Gmail…")).toBeVisible();
    finishBackfill();
    await waitFor(() => expect(screen.queryByText("Searching Gmail…")).not.toBeInTheDocument());
  });

  it("reports when Gmail search cannot be reached", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    vi.spyOn(mailClient, "backfillSearchThreads").mockRejectedValue(new Error("offline"));

    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search mail" });
    fireEvent.change(search, { target: { value: "126" } });
    fireEvent.click(await screen.findByRole("button", { name: "Include archived or trashed mail in search" }));

    expect(await screen.findByText("Gmail search unavailable")).toBeVisible();
  });
});
