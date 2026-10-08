import { describe, expect, it } from "vitest";
import type { ContactGroup, ContactGroupRecipients, ContactProfile } from "./domain";
import { GROUP_VISIBLE_RECIPIENT_LIMIT, MAX_GROUP_SUGGESTIONS, filterGroups, groupCandidates, groupMembers, groupsForContact, matchingGroups, memberCountLabel, typedAddress } from "./contactGroups";

const profile = (id: string, displayName: string | null, addresses: string[]): ContactProfile => ({
  id, displayName, role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null, favorite: false,
  addresses, sentCount: 0, receivedCount: 0, lastInteractedAt: null, birthday: null,
  keepInTouch: { intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null }, keepInTouchDueAt: null,
});
const group = (id: string, name: string, memberIds: string[]): ContactGroup => ({ id, name, memberIds, createdAt: "2026-10-07T00:00:00Z", updatedAt: "2026-10-07T00:00:00Z" });

const ada = profile("contact:ada", "Ada", ["ada@example.com"]);
const bob = profile("contact:bob", "Bob", ["bob@example.com", "bob@work.example"]);
const derivedBob = profile("derived:bob@work.example", "Bob", ["bob@work.example"]);
const cyd = profile("derived:cyd@example.com", null, ["cyd@example.com"]);

describe("contactGroups", () => {
  it("labels member counts", () => {
    expect(memberCountLabel(0)).toBe("0 members");
    expect(memberCountLabel(1)).toBe("1 member");
    expect(memberCountLabel(2)).toBe("2 members");
  });

  it("finds a contact's groups and filters groups by name", () => {
    const board = group("g1", "Board", [ada.id, bob.id]);
    const family = group("g2", "Family", [bob.id]);
    expect(groupsForContact([board, family], bob.id)).toEqual([board, family]);
    expect(groupsForContact([board, family], cyd.id)).toEqual([]);
    expect(filterGroups([board, family], " FAM ")).toEqual([family]);
    expect(filterGroups([board, family], "")).toEqual([board, family]);
  });

  it("keeps the group's member order and skips members whose contact isn't loaded", () => {
    const board = group("g1", "Board", [bob.id, "contact:not-here", ada.id]);
    expect(groupMembers(board, [ada, bob]).map((item) => item.id)).toEqual([bob.id, ada.id]);
  });

  it("offers everyone not in the group, but not a derived duplicate of a member's address", () => {
    const board = group("g1", "Board", [bob.id]);
    expect(groupCandidates(board, [ada, bob, derivedBob, cyd]).map((item) => item.id)).toEqual([ada.id, cyd.id]);
  });

  it("reads a typed address from plain or named input", () => {
    expect(typedAddress(" New@Example.com ")).toBe("new@example.com");
    expect(typedAddress("New Person <new@example.com>")).toBe("new@example.com");
    for (const invalid of ["", "new", "new@", "@example.com", "two words@example.com"]) expect(typedAddress(invalid), invalid).toBeNull();
  });

  it("suggests groups whose name contains the typed text, prefix matches first, skipping empty groups", () => {
    const member = { contactId: "c", displayName: null, email: "c@example.com", addresses: ["c@example.com"] };
    const named = (name: string, members = [member]): ContactGroupRecipients => ({ id: name, name, members });
    const groups = [named("Old Board"), named("Board"), named("Boardgames", []), named("Family")];
    expect(matchingGroups(groups, " board ").map((group) => group.name)).toEqual(["Board", "Old Board"]);
    expect(matchingGroups(groups, "")).toEqual([]);
    const many = Array.from({ length: MAX_GROUP_SUGGESTIONS + 2 }, (_, index) => named(`Team ${index}`));
    expect(matchingGroups(many, "team")).toHaveLength(MAX_GROUP_SUGGESTIONS);
  });

  it("warns above ten visible group members", () => {
    expect(GROUP_VISIBLE_RECIPIENT_LIMIT).toBe(10);
  });
});

