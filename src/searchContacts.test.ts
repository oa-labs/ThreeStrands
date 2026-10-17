import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { ContactProfile, ContactSuggestion } from "./domain";
import { loadSearchContacts, MAX_CONTACT_SEARCH_ADDRESSES, mergeSearchContacts } from "./searchContacts";

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
      { email: "pinned@example.com", name: null, addresses: ["pinned@example.com"] },
      { email: "Ana@example.com", name: "Ana", addresses: ["Ana@example.com"] },
      { email: "quiet@example.com", name: null, addresses: ["quiet@example.com"] },
    ]);
  });

  it("gives an address-book contact all their addresses, once", () => {
    const profile = { id: "ana", displayName: "Ana Diaz", addresses: ["ana@home.test", "ana@work.test", "a.diaz@old.test"] } as ContactProfile;
    const merged = mergeSearchContacts(
      [[contact("ana@work.test", { sentCount: 5 }), contact("ana@home.test", { sentCount: 1 }), contact("bo@example.com")]],
      [profile],
    );
    expect(merged).toEqual([
      // The address actually suggested leads; the rest follow.
      { email: "ana@work.test", name: "Ana Diaz", addresses: ["ana@work.test", "ana@home.test", "a.diaz@old.test"] },
      { email: "bo@example.com", name: null, addresses: ["bo@example.com"] },
    ]);
  });

  it("caps how many addresses one contact adds to a search", () => {
    const addresses = Array.from({ length: MAX_CONTACT_SEARCH_ADDRESSES + 3 }, (_, index) => `p${index}@example.com`);
    const [merged] = mergeSearchContacts([[contact("p0@example.com")]], [{ id: "p", displayName: "P", addresses } as ContactProfile]);
    expect(merged.addresses).toHaveLength(MAX_CONTACT_SEARCH_ADDRESSES);
  });

  it("asks every searched account and skips one that fails", async () => {
    const list = vi.spyOn(mailClient, "listContactSuggestions").mockImplementation(async (accountId) => {
      if (accountId === "down@example.com") throw new Error("offline");
      return [contact(`friend@${accountId.split("@")[1]}`)];
    });
    const found = await loadSearchContacts(["me@work.test", "down@example.com"], "fr");
    expect(list).toHaveBeenCalledWith("me@work.test", "fr", 8);
    expect(list).toHaveBeenCalledWith("down@example.com", "fr", 8);
    expect(found).toEqual([{ email: "friend@work.test", name: null, addresses: ["friend@work.test"] }]);
  });
});
