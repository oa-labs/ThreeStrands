import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";

function selectFolder(name: string) {
  fireEvent.click(screen.getByRole("button", { name: /Choose folder, current folder/ }));
  fireEvent.click(within(screen.getByRole("group", { name: "Folders" })).getByRole("button", { name }));
}

describe("split inbox search shortcuts", () => {
  afterEach(() => {
    cleanup();
    localStorage.removeItem("threestrands.settings.selectedAccountId");
    localStorage.removeItem("threestrands.settings.selectedMailboxByAccount");
    localStorage.removeItem("threestrands.settings.selectedTabByAccount");
    vi.restoreAllMocks();
  });

  it("shows Inbox as the folder and Main as the default tab, with a dismissible folder menu", async () => {
    render(<App />);
    expect(await screen.findByRole("tab", { name: /^Main/ })).toHaveAttribute("aria-selected", "true");
    const trigger = screen.getByRole("button", { name: "Choose folder, current folder Inbox" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("navigation", { name: "Mailboxes" }).querySelectorAll(".sidebar-nav")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "New message (c)" }).closest(".sidebar-nav")).toBeInTheDocument();

    fireEvent.click(trigger);
    const folders = screen.getByRole("group", { name: "Folders" });
    expect(within(folders).getByRole("button", { name: "Inbox" })).toHaveAttribute("aria-current", "page");
    expect(within(folders).getAllByRole("button").map((button) => button.textContent)).toEqual(
      expect.arrayContaining([expect.stringContaining("All Mail"), expect.stringContaining("Drafts"), expect.stringContaining("Outbox"), expect.stringContaining("Trash")]),
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("group", { name: "Folders" })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
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
    expect(screen.getByRole("button", { name: "Choose folder, current folder Inbox" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
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
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Main/ })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("textbox", { name: "Search Mail" })).toHaveValue("roadmap");
    expect(screen.getByRole("textbox", { name: "Search Mail" })).toHaveFocus();

    fireEvent.click(splitTab);
    await waitFor(() => expect(splitTab).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("textbox", { name: "Search Mail" })).toHaveValue("roadmap");

    fireEvent.keyDown(screen.getByRole("textbox", { name: "Search Mail" }), { key: "Tab", shiftKey: true });
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Main/ })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("textbox", { name: "Search Mail" })).toHaveValue("roadmap");
  });

  it("restores each account's last folder after keyboard and mouse account switches", async () => {
    const [primary] = await mailClient.listAccounts();
    vi.spyOn(mailClient, "listAccounts").mockResolvedValue([
      primary!,
      { ...primary!, email: "work@example.com", displayName: "Work", color: "#34A853", sortOrder: 1 },
    ]);
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");

    render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    selectFolder("Outbox");
    expect(screen.getByRole("button", { name: "Choose folder, current folder Outbox" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "2", ctrlKey: true });
    await waitFor(() => expect(screen.getByRole("radio", { name: "Work" })).toHaveAttribute("aria-checked", "true"));
    selectFolder("All Mail");
    expect(screen.getByRole("button", { name: "Choose folder, current folder All Mail" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: /^demo@example\.com/ }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Choose folder, current folder Outbox" })).toBeInTheDocument());

    fireEvent.keyDown(window, { key: "2", ctrlKey: true });
    await waitFor(() => expect(screen.getByRole("button", { name: "Choose folder, current folder All Mail" })).toBeInTheDocument());
  });

  it("closes the search box when switching accounts, but not when switching tabs", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    const [primary] = await mailClient.listAccounts();
    vi.spyOn(mailClient, "listAccounts").mockResolvedValue([
      primary!,
      {
        ...primary!,
        email: "work@example.com",
        displayName: "Work",
        color: "#34A853",
        sortOrder: 1,
      },
    ]);
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
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "roadmap" } });

    fireEvent.click(splitTab);
    await waitFor(() => expect(splitTab).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("textbox", { name: "Search Mail" })).toHaveValue("roadmap");

    fireEvent.click(screen.getByRole("radio", { name: "Work" }));
    await waitFor(() => expect(screen.getByRole("radio", { name: "Work" })).toHaveAttribute("aria-checked", "true"));
    expect(screen.queryByRole("textbox", { name: "Search Mail" })).not.toBeInTheDocument();
  });

  it("starts every folder outside the tab bar with a closed, empty search", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    render(<App />);
    await screen.findByRole("tab", { name: /^Main/ });

    for (const folder of ["All Mail", "Trash", "Drafts", "Outbox"]) {
      fireEvent.keyDown(window, { key: "/" });
      fireEvent.change(await screen.findByRole("textbox", { name: "Search Mail" }), { target: { value: "roadmap" } });

      selectFolder(folder);
      await waitFor(() => expect(screen.getByRole("button", { name: `Choose folder, current folder ${folder}` })).toBeInTheDocument());
      expect(screen.queryByRole("textbox", { name: "Search Mail" })).not.toBeInTheDocument();

      selectFolder("Inbox");
      await screen.findByRole("tab", { name: /^Main/ });
    }
  });

  it("remembers the chosen Inbox or split tab for the account", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
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
    const rememberedTab = () =>
      JSON.parse(localStorage.getItem("threestrands.settings.selectedTabByAccount") ?? "{}")["demo@example.com"];

    render(<App />);
    const splitTab = await screen.findByRole("tab", { name: "Work" });
    fireEvent.click(splitTab);
    await waitFor(() => expect(splitTab).toHaveAttribute("aria-selected", "true"));
    expect(rememberedTab()).toBe("work-split");

    fireEvent.click(screen.getByRole("tab", { name: /^Main/ }));
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Main/ })).toHaveAttribute("aria-selected", "true"));
    expect(rememberedTab()).toBeNull();
  });

  it("shows when the Gmail backfill is still running", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    let finishBackfill!: () => void;
    const backfillSearchThreads = vi.spyOn(mailClient, "backfillSearchThreads").mockImplementation(
      () => new Promise<void>((resolve) => { finishBackfill = resolve; }),
    );

    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
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
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "126" } });
    fireEvent.click(await screen.findByRole("button", { name: "Include archived or trashed mail in search" }));

    expect(await screen.findByText("Gmail search unavailable")).toBeVisible();
  });
});
