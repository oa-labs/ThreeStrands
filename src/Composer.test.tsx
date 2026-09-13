import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Composer } from "./Composer";
import type { Draft } from "./correspondence";
import { mailClient } from "./data/client";
import type { Account } from "./domain";

const draft: Draft = {
  id: "draft-1",
  revision: 0,
  account: "first@example.com",
  mode: "new",
  sourceId: null,
  threadId: null,
  replyId: null,
  references: [],
  to: "",
  cc: "",
  bcc: "",
  subject: "",
  body: "",
  attachments: [],
  updatedAt: 0,
};

const accounts: Account[] = ["first@example.com", "second@example.com"].map((email, sortOrder) => ({
  email,
  displayName: null,
  color: "#4285F4",
  status: "connected",
  sortOrder,
  connectedAt: "2026-01-01T00:00:00Z",
  lastSyncedAt: null,
}));

describe("Composer From selector", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("keeps native pointer interaction and changes the sending account inline", async () => {
    vi.spyOn(mailClient, "setDraftAccount").mockResolvedValue({
      ...draft,
      revision: 1,
      account: "second@example.com",
    });

    render(<Composer draft={draft} accounts={accounts} onClose={() => {}} onQueued={() => {}} />);
    const selector = screen.getByRole("combobox", { name: "Send from" });

    expect(fireEvent.pointerDown(selector, { button: 0, pointerId: 1 })).toBe(true);
    fireEvent.change(selector, { target: { value: "second@example.com" } });

    await waitFor(() => expect(selector).toHaveValue("second@example.com"));
    expect(mailClient.setDraftAccount).toHaveBeenCalledWith("draft-1", "second@example.com");
  });
});
