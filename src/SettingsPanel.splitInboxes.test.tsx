import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SplitInbox } from "./domain";
import { SettingsHarness } from "./test/settingsHarness";
import { SplitInboxesSettings } from "./SplitInboxesSettings";

afterEach(cleanup);

const splitInbox: SplitInbox = {
  id: "upward",
  name: "Upward",
  matchKind: "domain",
  matchValue: "upwardprojects.com",
  accountId: "joelreed@openarc.net",
  sortOrder: 0,
  createdAt: "2026-09-30T12:00:00Z",
};

function renderSettings(onDelete = vi.fn<(id: string) => Promise<void>>().mockResolvedValue(undefined), splitInboxes = [splitInbox]) {
  render(
    <SplitInboxesSettings
      showStarredInbox={true}
      onShowStarredInboxChange={vi.fn()}
      splitInboxes={splitInboxes}
      accounts={[]}
      activeAccountId={null}
      labelsByAccount={{}}
      onCreate={vi.fn()}
      onRename={vi.fn()}
      onDelete={onDelete}
      onReorder={vi.fn()}
    />,
  );
  return onDelete;
}

describe("split inbox settings cards", () => {
  it("persists the automatic Starred inbox opt-out", () => {
    localStorage.removeItem("threestrands.settings.showStarredInbox");
    const first = render(<SettingsHarness initialSection="splitInboxes" />);
    const toggle = screen.getByRole("checkbox", { name: "Show Starred inbox automatically" });
    expect(toggle).toBeChecked();
    expect(toggle).toHaveAccessibleDescription();
    fireEvent.click(toggle);
    expect(toggle).not.toBeChecked();
    first.unmount();
    render(<SettingsHarness initialSection="splitInboxes" />);
    expect(screen.getByRole("checkbox", { name: "Show Starred inbox automatically" })).not.toBeChecked();
    localStorage.removeItem("threestrands.settings.showStarredInbox");
  });

  it("starts the name and rule at the same card edge as the actions", () => {
    renderSettings();

    const card = screen.getByRole("listitem");
    const content = card.querySelector(".account-card-row");
    expect(content?.firstElementChild).toHaveClass("account-card-identity");
    expect(card.querySelector(".account-card-avatar")).toBeNull();
    expect(within(card).getByRole("textbox", { name: "Name for Upward" })).toHaveValue("Upward");
    expect(card).toHaveTextContent("Sending domain: upwardprojects.com — joelreed@openarc.net");
    expect(within(card).getByRole("button", { name: "Delete" })).toBeInTheDocument();
  });

  it("requires confirmation and allows cancellation without deleting", () => {
    const onDelete = renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    const confirmation = screen.getByRole("group", { name: "Delete Upward confirmation" });
    expect(confirmation).toHaveTextContent("Delete Upward?");
    expect(confirmation).toHaveTextContent("Your emails will not be deleted.");
    expect(onDelete).not.toHaveBeenCalled();

    fireEvent.click(within(confirmation).getByRole("button", { name: "Cancel" }));

    expect(screen.queryByRole("group", { name: "Delete Upward confirmation" })).not.toBeInTheDocument();
    expect(onDelete).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(screen.getByRole("group", { name: "Delete Upward confirmation" })).toBeInTheDocument();
    expect(onDelete).not.toHaveBeenCalled();
  });

  it("deletes only the confirmed inbox and prevents another deletion while pending", async () => {
    let finishDelete!: () => void;
    const onDelete = renderSettings(
      vi.fn<(id: string) => Promise<void>>().mockImplementation(() => new Promise<void>((resolve) => { finishDelete = resolve; })),
      [splitInbox, { ...splitInbox, id: "clients", name: "Clients", sortOrder: 1 }],
    );
    const cards = screen.getAllByRole("listitem");
    fireEvent.click(within(cards[0]!).getByRole("button", { name: "Delete" }));
    fireEvent.click(within(cards[1]!).getByRole("button", { name: "Delete" }));

    expect(screen.queryByRole("group", { name: "Delete Upward confirmation" })).not.toBeInTheDocument();
    const confirmation = screen.getByRole("group", { name: "Delete Clients confirmation" });
    expect(onDelete).not.toHaveBeenCalled();

    fireEvent.click(within(confirmation).getByRole("button", { name: "Delete Split Inbox" }));

    expect(onDelete).toHaveBeenCalledExactlyOnceWith("clients");
    expect(screen.queryByRole("group", { name: "Delete Clients confirmation" })).not.toBeInTheDocument();
    for (const button of screen.getAllByRole("button", { name: "Delete" })) {
      expect(button).toBeDisabled();
      fireEvent.click(button);
    }
    expect(onDelete).toHaveBeenCalledTimes(1);
    await act(async () => finishDelete());
    expect(within(cards[0]!).getByRole("button", { name: "Delete" })).toBeEnabled();
  });

  it("reports deletion failures and requires confirmation again to retry", async () => {
    const onDelete = renderSettings(vi.fn<(id: string) => Promise<void>>().mockRejectedValue(new Error("Could not delete split inbox")));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete Split Inbox" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not delete split inbox");
    expect(screen.getByRole("button", { name: "Delete" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Delete Split Inbox" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(screen.getByRole("group", { name: "Delete Upward confirmation" })).toBeInTheDocument();
    expect(onDelete).toHaveBeenCalledExactlyOnceWith("upward");
  });
});

describe("saved searches in Split Inboxes", () => {
  const saved = { id: "legacy", name: "Bills", query: "invoice", accountId: "old@example.com", inboxOnly: true, messageFilters: ["unread" as const] };
  const view = { id: "saved-search:legacy", name: saved.name, accountId: saved.accountId, savedSearch: saved };
  function renderView() {
    const update = vi.fn().mockResolvedValue(undefined), remove = vi.fn().mockResolvedValue(undefined), create = vi.fn().mockResolvedValue(undefined);
    render(<SplitInboxesSettings splitInboxes={[view]} showStarredInbox onShowStarredInboxChange={vi.fn()} accounts={[]} activeAccountId={null} labelsByAccount={{}}
      onCreate={vi.fn()} onCreateSearch={create} onUpdateSearch={update} onRename={vi.fn()} onDelete={remove} onReorder={vi.fn()} />);
    return { update, remove, create };
  }
  it("keeps disconnected scope explicit and edits all saved search conditions", async () => {
    const { update } = renderView();
    expect(screen.getByRole("listitem")).toHaveTextContent("Account disconnected");
    fireEvent.click(screen.getByRole("button", { name: "Edit Bills" }));
    const dialog = screen.getByRole("dialog", { name: "Edit Saved Search" });
    expect(within(dialog).getByRole("combobox", { name: "Account scope" })).toHaveValue("old@example.com");
    expect(within(dialog).getByRole("button", { name: "Save Search" })).toBeDisabled();
    fireEvent.change(within(dialog).getByRole("combobox"), { target: { value: "" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "All bills" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Search" }), { target: { value: "has:attachment" } });
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Inbox Only" }));
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Unread" }));
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Starred" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Search" }));
    expect(update).toHaveBeenCalledWith(view.id, { name: "All bills", query: "has:attachment", accountId: null, inboxOnly: false, messageFilters: ["starred"] });
    await screen.findByRole("button", { name: "Edit Bills" });
  });
  it("deletes a saved view using the shared confirmation without touching mail", () => {
    const { remove } = renderView();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const confirmation = screen.getByRole("group", { name: "Delete Bills confirmation" });
    expect(confirmation).toHaveTextContent("Your emails will not be deleted.");
    fireEvent.click(within(confirmation).getByRole("button", { name: "Delete Split Inbox" }));
    expect(remove).toHaveBeenCalledWith(view.id);
  });
  it("creates a saved search from the same settings section with explicit All accounts scope", async () => {
    const { create } = renderView();
    fireEvent.click(screen.getByRole("button", { name: "Add Saved Search" }));
    const dialog = screen.getByRole("dialog", { name: "New Saved Search" });
    expect(within(dialog).getByRole("combobox", { name: "Account scope" })).toHaveValue("");
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "Files" } });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Search" }), { target: { value: "has:attachment" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Search" }));
    expect(create).toHaveBeenCalledWith({ name: "Files", query: "has:attachment", accountId: null, inboxOnly: false, messageFilters: [] });
    await screen.findByRole("button", { name: "Add Saved Search" });
  });
});
