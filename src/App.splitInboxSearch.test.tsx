import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";
import type { SearchReport } from "./domain";
import { REMOTE_SEARCH_DELAY_MS } from "./useMailboxThreads";

// Server search starts only after the query holds still, so waits for it
// allow for the typing debounce plus that pause.
const SERVER_SEARCH_WAIT = { timeout: REMOTE_SEARCH_DELAY_MS + 2000 };

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

  it.each([
    ["Inbox", "listThreadsPage"],
    ["All Mail", "listAllMailPage"],
    ["Sent", "listSentPage"],
    ["Trash", "listTrashPage"],
    ["Work", "listSplitInboxPage"],
  ] as const)("uses the same page source for initial and subsequent %s results", async (folder, method) => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    const threads = await mailClient.listThreads();
    expect(threads.length).toBeGreaterThanOrEqual(3);
    const fetchPage = vi.spyOn(mailClient, method)
      .mockResolvedValueOnce({ threads: threads.slice(0, 2), hasMore: true })
      .mockResolvedValueOnce({ threads: threads.slice(2, 3), hasMore: false });
    vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([{
      id: "work-split", name: "Work", matchKind: "label", matchValue: "work", sortOrder: 0,
      createdAt: "2026-03-01T00:00:00Z", accountId: "demo@example.com",
    }]);

    render(<App />);
    await screen.findByRole("tab", { name: "Work" });
    if (folder === "Work") fireEvent.click(screen.getByRole("tab", { name: "Work" }));
    else if (folder !== "Inbox") selectFolder(folder);

    const sourceId = folder === "Work" ? "work-split" : "demo@example.com";
    await waitFor(() => expect(fetchPage).toHaveBeenCalledWith(sourceId, 0, 50));
    const list = screen.getByRole("listbox", { name: folder });
    await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(2));
    fireEvent.click(screen.getByRole("button", { name: "Load More Results" }));

    await waitFor(() => expect(fetchPage).toHaveBeenCalledWith(sourceId, 2, 50));
    await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(3));
    expect(screen.queryByRole("button", { name: "Load More Results" })).not.toBeInTheDocument();
  });

  it("shows Inbox as the folder and Main as the default tab, with a dismissible folder menu", async () => {
    vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([{
      id: "work-split", name: "Work", matchKind: "label", matchValue: "work", sortOrder: 0,
      createdAt: "2026-03-01T00:00:00Z", accountId: "demo@example.com",
    }]);
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
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

  it("hides the lone Main tab until a split exists, keeping Add Split and Filters on the title row", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    const listSplits = vi.spyOn(mailClient, "listSplitInboxes").mockResolvedValue([]);
    const first = render(<App />);
    const addSplit = await screen.findByRole("button", { name: "Add Split Inbox" });
    expect(screen.queryByRole("tablist", { name: "Mailbox views" })).not.toBeInTheDocument();
    const titleRow = addSplit.closest(".mailbox-title-row") as HTMLElement;
    expect(within(titleRow).getByRole("heading", { level: 1 })).toBeInTheDocument();
    expect(within(titleRow).getByRole("button", { name: "Filters" })).toBeInTheDocument();
    first.unmount();

    listSplits.mockResolvedValue([{
      id: "work-split", name: "Work", matchKind: "label", matchValue: "work", sortOrder: 0,
      createdAt: "2026-03-01T00:00:00Z", accountId: "demo@example.com",
    }]);
    render(<App />);
    const tabs = await screen.findByRole("tablist", { name: "Mailbox views" });
    expect(within(tabs).getAllByRole("tab").map((tab) => tab.textContent)).toEqual([expect.stringMatching(/^Main/), "Work"]);
    expect(within(tabs).queryByRole("button", { name: "Add Split Inbox" })).not.toBeInTheDocument();
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
    await waitFor(
      () => expect(backfillSearchThreads).toHaveBeenCalledWith("roadmap", "demo@example.com"),
      SERVER_SEARCH_WAIT,
    );

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

  it("closes the search box when switching accounts", async () => {
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
    await screen.findByRole("tab", { name: "Work" });
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "roadmap" } });
    expect(search).toHaveValue("roadmap");

    fireEvent.click(screen.getByRole("radio", { name: "Work" }));
    await waitFor(() => expect(screen.getByRole("radio", { name: "Work" })).toHaveAttribute("aria-checked", "true"));
    expect(screen.queryByRole("textbox", { name: "Search Mail" })).not.toBeInTheDocument();
  });

  it("starts every folder outside the tab bar with a closed, empty search", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    render(<App />);
    await screen.findByRole("button", { name: "Choose folder, current folder Inbox" });

    for (const folder of ["All Mail", "Trash", "Drafts", "Outbox"]) {
      fireEvent.keyDown(window, { key: "/" });
      fireEvent.change(await screen.findByRole("textbox", { name: "Search Mail" }), { target: { value: "roadmap" } });

      selectFolder(folder);
      await waitFor(() => expect(screen.getByRole("button", { name: `Choose folder, current folder ${folder}` })).toBeInTheDocument());
      expect(screen.queryByRole("textbox", { name: "Search Mail" })).not.toBeInTheDocument();

      selectFolder("Inbox");
      await screen.findByRole("button", { name: "Choose folder, current folder Inbox" });
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
    const first = render(<App />);
    const splitTab = await screen.findByRole("tab", { name: "Work" });
    fireEvent.click(splitTab);
    await waitFor(() => expect(splitTab).toHaveAttribute("aria-selected", "true"));
    first.unmount();

    const second = render(<App />);
    await waitFor(() => expect(screen.getByRole("tab", { name: "Work" })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("tab", { name: /^Main/ })).toHaveAttribute("aria-selected", "false");

    fireEvent.click(screen.getByRole("tab", { name: /^Main/ }));
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Main/ })).toHaveAttribute("aria-selected", "true"));
    second.unmount();

    render(<App />);
    await screen.findByRole("tab", { name: "Work" });
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Main/ })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("tab", { name: "Work" })).toHaveAttribute("aria-selected", "false");
  });

  it("keeps the active split when returning from calendar, tasks, and contacts", async () => {
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

    render(<App />);
    const splitTab = await screen.findByRole("tab", { name: "Work" });
    fireEvent.click(splitTab);
    await waitFor(() => expect(splitTab).toHaveAttribute("aria-selected", "true"));

    for (const view of ["Calendar (2)", "Tasks (3)", "Contacts (4)"]) {
      fireEvent.click(screen.getByRole("button", { name: view }));
      expect(screen.getByRole("button", { name: view })).toHaveClass("active");
      fireEvent.click(screen.getByRole("button", { name: "Inbox (1)" }));
      expect(screen.getByRole("tab", { name: "Work" })).toHaveAttribute("aria-selected", "true");
      expect(screen.getByRole("button", { name: "Choose folder, current folder Inbox" })).toBeInTheDocument();
    }
  });

  it("searches all mail by default and only the inbox when narrowed", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    const searchThreads = vi.spyOn(mailClient, "searchThreads");
    const backfill = vi.spyOn(mailClient, "backfillSearchThreads");
    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "roadmap" } });
    await waitFor(() => expect(searchThreads).toHaveBeenCalledWith(
      expect.objectContaining({ query: "roadmap", includeArchived: true }),
      "demo@example.com",
    ));
    await waitFor(() => expect(backfill).toHaveBeenCalledWith("roadmap", "demo@example.com"), SERVER_SEARCH_WAIT);

    searchThreads.mockClear();
    backfill.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Search only the Inbox" }));
    await waitFor(() => expect(searchThreads).toHaveBeenCalledWith(
      expect.objectContaining({ query: "roadmap", includeArchived: false }),
      "demo@example.com",
    ));
    await new Promise((resolve) => window.setTimeout(resolve, REMOTE_SEARCH_DELAY_MS + 100));
    expect(backfill).not.toHaveBeenCalled();
  });

  it("waits for the query to hold still before searching the server", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    const searchThreads = vi.spyOn(mailClient, "searchThreads");
    const backfill = vi.spyOn(mailClient, "backfillSearchThreads");
    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "road" } });
    // The local search for the first query lands right away...
    await waitFor(() => expect(searchThreads).toHaveBeenCalledWith(
      expect.objectContaining({ query: "road" }),
      "demo@example.com",
    ));
    expect(backfill).not.toHaveBeenCalled();
    // ...but the user keeps typing before the pause ends, so only the final
    // query reaches the server.
    fireEvent.change(search, { target: { value: "roadmap" } });
    await waitFor(() => expect(backfill).toHaveBeenCalledWith("roadmap", "demo@example.com"), SERVER_SEARCH_WAIT);
    expect(backfill).toHaveBeenCalledTimes(1);
  });

  it("autocompletes contacts and labels from the account being searched", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    vi.spyOn(mailClient, "listLabels").mockResolvedValue([
      { id: "INBOX", name: "INBOX", kind: "system" },
      { id: "Label_9", name: "Receipts", kind: "user" },
    ]);
    const contacts = vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([
      { email: "ana@example.com", displayName: "Ana Diaz", sentCount: 4, receivedCount: 2, lastInteractedAt: "", pinned: false },
    ]);
    const searchThreads = vi.spyOn(mailClient, "searchThreads");
    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });

    fireEvent.change(search, { target: { value: "from:an" } });
    fireEvent.click(await screen.findByRole("option", { name: /Ana Diaz/ }));
    expect(contacts).toHaveBeenCalledWith("demo@example.com", "an", 8);
    expect(search).toHaveValue("from:ana@example.com ");

    fireEvent.change(search, { target: { value: "from:ana@example.com label:" } });
    // System labels are not offered; the account's own labels are.
    expect(await screen.findByRole("option", { name: "Receipts" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "INBOX" })).not.toBeInTheDocument();
    fireEvent.keyDown(search, { key: "Enter" });
    expect(search).toHaveValue("from:ana@example.com label:Receipts ");
    await waitFor(() => expect(searchThreads).toHaveBeenCalledWith(
      expect.objectContaining({ query: "from:ana@example.com label:Receipts" }),
      "demo@example.com",
    ));
  });

  it("shows when the Gmail backfill is still running", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    let finishBackfill!: () => void;
    const backfillSearchThreads = vi.spyOn(mailClient, "backfillSearchThreads").mockImplementation(
      () => new Promise<SearchReport>((resolve) => {
        finishBackfill = () => resolve({ status: { kind: "complete" }, unsyncedMailboxes: 0, skippedOverCap: 0, serverSearchedAccounts: 1 });
      }),
    );

    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "126" } });

    await waitFor(() => expect(backfillSearchThreads).toHaveBeenCalledWith("126", "demo@example.com"), SERVER_SEARCH_WAIT);
    expect(await screen.findByText("Searching mail on the server…")).toBeVisible();
    finishBackfill();
    await waitFor(() => expect(screen.queryByText("Searching mail on the server…")).not.toBeInTheDocument());
  });

  it("reports when server search cannot be reached", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    vi.spyOn(mailClient, "backfillSearchThreads").mockRejectedValue(new Error("offline"));

    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "126" } });

    expect(await screen.findByText("Server search unavailable. Showing mail on this device.", undefined, SERVER_SEARCH_WAIT)).toBeVisible();
  });
});

describe("server-search status toast (Phase 5 run 5)", () => {
  afterEach(() => {
    cleanup();
    localStorage.removeItem("threestrands.settings.selectedAccountId");
    vi.restoreAllMocks();
  });

  const complete: SearchReport = { status: { kind: "complete" }, unsyncedMailboxes: 0, skippedOverCap: 0, serverSearchedAccounts: 1 };
  const skipped: SearchReport = { status: { kind: "skipped", reasons: [{ code: "unsupported" }] }, unsyncedMailboxes: 0, skippedOverCap: 0, serverSearchedAccounts: 0 };
  const partialDeadline: SearchReport = { status: { kind: "partial", reasons: [{ code: "deadline" }] }, unsyncedMailboxes: 0, skippedOverCap: 0, serverSearchedAccounts: 1 };

  async function searchWithReport(report: SearchReport, query = "roadmap") {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    const backfill = vi.spyOn(mailClient, "backfillSearchThreads").mockResolvedValue(report);
    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: query } });
    const toggle = await screen.findByRole("button", { name: "Search only the Inbox" });
    await waitFor(() => expect(backfill).toHaveBeenCalledWith(query, "demo@example.com"), SERVER_SEARCH_WAIT);
    return { search, toggle };
  }

  it("shows the skipped copy with a screen-reader status role", async () => {
    await searchWithReport(skipped);
    const toast = await screen.findByText(/only mail already on this device is shown/);
    expect(toast).toBeVisible();
    const region = toast.closest("[role=status]");
    expect(region).not.toBeNull();
    expect(region).toHaveAttribute("aria-live", "polite");
  });

  it("shows the deadline partial copy", async () => {
    await searchWithReport(partialDeadline);
    expect(await screen.findByText("Some folders took too long to search. Showing what was found.")).toBeVisible();
  });

  it("shows nothing new for a complete search", async () => {
    await searchWithReport(complete);
    // The two existing toasts are gone (idle), and no status line appears.
    await waitFor(() => expect(screen.queryByText("Searching mail on the server…")).not.toBeInTheDocument());
    expect(screen.queryByText(/only mail already on this device/)).not.toBeInTheDocument();
    expect(screen.queryByText(/took too long to search/)).not.toBeInTheDocument();
  });

  it("clears the status line when the query changes", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    // First query comes back skipped; a different query comes back complete,
    // so the skipped line must clear and not reappear.
    const backfill = vi.spyOn(mailClient, "backfillSearchThreads")
      .mockResolvedValueOnce(skipped)
      .mockResolvedValue(complete);
    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "roadmap" } });
    await waitFor(() => expect(backfill).toHaveBeenCalledWith("roadmap", "demo@example.com"), SERVER_SEARCH_WAIT);
    expect(await screen.findByText(/only mail already on this device is shown/)).toBeVisible();

    fireEvent.change(search, { target: { value: "something else" } });
    await waitFor(() => expect(screen.queryByText(/only mail already on this device is shown/)).not.toBeInTheDocument());
  });

  it("clears the status line when the search is narrowed to the inbox", async () => {
    const { toggle } = await searchWithReport(skipped);
    expect(await screen.findByText(/only mail already on this device is shown/)).toBeVisible();
    // The toggle keeps its accessible name; aria-pressed flips. Narrowing to
    // the inbox clears the status and runs no further server search.
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    await waitFor(() => expect(screen.queryByText(/only mail already on this device is shown/)).not.toBeInTheDocument());
  });

  it("is dismissible", async () => {
    await searchWithReport(partialDeadline);
    const toast = await screen.findByText("Some folders took too long to search. Showing what was found.");
    fireEvent.click(within(toast.closest("[role=status]") as HTMLElement).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(screen.queryByText("Some folders took too long to search. Showing what was found.")).not.toBeInTheDocument());
  });

  it("never shows a stale earlier query's report for the current one", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "demo@example.com");
    let resolveFirst!: (report: SearchReport) => void;
    const backfill = vi.spyOn(mailClient, "backfillSearchThreads")
      // First query's backfill hangs, then we switch query; its late report
      // (skipped) must not paint for the second query.
      .mockImplementationOnce(() => new Promise<SearchReport>((resolve) => { resolveFirst = resolve; }))
      .mockResolvedValueOnce(partialDeadline);

    render(<App />);
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "first" } });
    await waitFor(() => expect(backfill).toHaveBeenCalledWith("first", "demo@example.com"), SERVER_SEARCH_WAIT);

    // Switch to a second query; its backfill resolves with a partial.
    fireEvent.change(search, { target: { value: "second" } });
    await waitFor(() => expect(backfill).toHaveBeenCalledWith("second", "demo@example.com"), SERVER_SEARCH_WAIT);
    expect(await screen.findByText("Some folders took too long to search. Showing what was found.")).toBeVisible();

    // Now the FIRST query's backfill finally resolves with a skipped report.
    // Its request id is stale, so the skipped copy must never appear.
    resolveFirst(skipped);
    await waitFor(() => expect(backfill).toHaveBeenCalledTimes(2));
    expect(screen.queryByText(/only mail already on this device is shown/)).not.toBeInTheDocument();
    // The current query's partial line is still what's shown.
    expect(screen.getByText("Some folders took too long to search. Showing what was found.")).toBeVisible();
  });
});
