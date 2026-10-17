import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { ContactSuggestion } from "./domain";
import { loadSearchContacts, mergeSearchContacts } from "./searchContacts";

const contact = (email: string, overrides: Partial<ContactSuggestion> = {}): ContactSuggestion => ({
  email,
  displayName: null,
  sentCount: 0,
  receivedCount: 0,
  lastInteractedAt: "",
  pinned: false,
  ...overrides,
});

describe("search contact suggestions", () => {
  afterEach(() => vi.restoreAllMocks());

  it("ranks pinned contacts, then the most mail exchanged, one entry per address", () => {
    const merged = mergeSearchContacts([
      [contact("quiet@example.com", { sentCount: 1 }), contact("Ana@example.com", { displayName: "Ana", sentCount: 9 })],
      [contact("ana@example.com", { sentCount: 2 }), contact("pinned@example.com", { pinned: true })],
    ]);
    expect(merged).toEqual([
      { email: "pinned@example.com", name: null },
      { email: "Ana@example.com", name: "Ana" },
      { email: "quiet@example.com", name: null },
    ]);
  });

  it("asks every searched account and skips one that fails", async () => {
    const list = vi.spyOn(mailClient, "listContactSuggestions").mockImplementation(async (accountId) => {
      if (accountId === "down@example.com") throw new Error("offline");
      return [contact(`friend@${accountId.split("@")[1]}`)];
    });
    const found = await loadSearchContacts(["me@work.test", "down@example.com"], "fr");
    expect(list).toHaveBeenCalledWith("me@work.test", "fr", 8);
    expect(list).toHaveBeenCalledWith("down@example.com", "fr", 8);
    expect(found).toEqual([{ email: "friend@work.test", name: null }]);
  });
});
