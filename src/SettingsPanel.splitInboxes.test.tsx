import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SplitInbox } from "./domain";
import { SplitInboxesSettings } from "./SettingsPanel";

afterEach(cleanup);

describe("split inbox settings cards", () => {
  it("starts the name and rule at the same card edge as the actions", () => {
    const splitInbox: SplitInbox = {
      id: "upward",
      name: "Upward",
      matchKind: "domain",
      matchValue: "upwardprojects.com",
      accountId: "joelreed@openarc.net",
      sortOrder: 0,
      createdAt: "2026-09-30T12:00:00Z",
    };
    render(
      <SplitInboxesSettings
        splitInboxes={[splitInbox]}
        accounts={[]}
        activeAccountId={null}
        labelsByAccount={{}}
        onCreate={vi.fn()}
        onRename={vi.fn()}
        onDelete={vi.fn()}
        onReorder={vi.fn()}
      />,
    );

    const card = screen.getByRole("listitem");
    const content = card.querySelector(".account-card-row");
    expect(content?.firstElementChild).toHaveClass("account-card-identity");
    expect(card.querySelector(".account-card-avatar")).toBeNull();
    expect(within(card).getByRole("textbox", { name: "Name for Upward" })).toHaveValue("Upward");
    expect(card).toHaveTextContent("Sending domain: upwardprojects.com — joelreed@openarc.net");
    expect(within(card).getByRole("button", { name: "Delete" })).toBeInTheDocument();
  });
});
