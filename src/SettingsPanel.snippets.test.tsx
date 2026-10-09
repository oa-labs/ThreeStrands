import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Snippet } from "./domain";
import { SnippetsSettings } from "./SnippetsSettings";

afterEach(cleanup);

describe("snippet settings cards", () => {
  it("starts the name and preview at the same card edge as the actions", () => {
    const snippet: Snippet = {
      id: "meet",
      name: "Meet",
      body: "<p>https://cal.com/joelreed/meet</p>",
      createdAt: "2026-09-30T12:00:00Z",
    };
    render(
      <SnippetsSettings
        snippets={[snippet]}
        onCreate={vi.fn()}
        onUpdate={vi.fn()}
        onDelete={vi.fn()}
      />,
    );

    const card = screen.getByRole("listitem");
    const content = card.querySelector(".account-card-row");
    expect(content?.firstElementChild).toHaveClass("account-card-identity");
    expect(card.querySelector(".account-card-avatar")).toBeNull();
    expect(within(card).getByText("Meet")).toBeInTheDocument();
    expect(within(card).getByText("https://cal.com/joelreed/meet")).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Edit" })).toBeInTheDocument();
  });
});
