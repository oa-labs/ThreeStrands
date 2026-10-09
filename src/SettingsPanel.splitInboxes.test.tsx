import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SplitInbox } from "./domain";
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
