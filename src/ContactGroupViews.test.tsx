import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ContactMemberPicker } from "./ContactGroupViews";
import { mailClient } from "./data/client";
import type { ContactProfile, ContactGroup, ContactGroupSuggestion } from "./domain";

const profile = (id: string, name: string): ContactProfile => ({ id, displayName: name, addresses: [`${id}@example.com`], favorite: false,
  role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null,
  sentCount: 0, receivedCount: 0, lastInteractedAt: null, birthday: null,
  keepInTouch: { intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null }, keepInTouchDueAt: null });
const profiles = [profile("alice", "Alice"), profile("carol", "Carol"), { ...profile("bob", "Bob"), addresses: ["bob@example.com", "bob-work@example.com"] }];
const group: ContactGroup = { id: "team", name: "Team", memberIds: ["alice"], createdAt: "", updatedAt: "" };
const tick = () => act(async () => { await vi.advanceTimersByTimeAsync(150); });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }

describe("group member recommendations", () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });
  const list = () => within(screen.getByRole("listbox", { name: "Contacts" }));

  it("ranks recommendations without selecting them or changing primary addresses", async () => {
    const rank = vi.spyOn(mailClient, "rankContactGroupCandidates").mockResolvedValue([{ id: "bob", relatedTo: ["alice@example.com"] }]);
    const add = vi.fn().mockResolvedValue(undefined);
    render(<ContactMemberPicker group={group} profiles={profiles} onAdd={add} onClose={() => {}} />);
    await tick();
    expect(screen.getByRole("combobox", { name: "Find contacts to add" })).toHaveAccessibleDescription();
    expect(rank).toHaveBeenLastCalledWith("team", [], []);
    expect(list().getAllByRole("option")[0]).toHaveAccessibleName("Bob");
    expect(list().getByRole("option", { name: "Bob" })).toHaveTextContent("bob@example.com");
    expect(screen.getByRole("button", { name: "Add Members" })).toBeDisabled();
    expect(add).not.toHaveBeenCalled();
    fireEvent.click(list().getByRole("option", { name: "Bob" }));
    await tick(); expect(rank).toHaveBeenLastCalledWith("team", ["bob"], []);
    fireEvent.click(screen.getByRole("button", { name: "Add 1 Member" }));
    expect(add).toHaveBeenCalledWith(["bob"], []);
  });

  it("ignores old responses after selection and retains the highlighted person on reorder", async () => {
    const old = deferred<ContactGroupSuggestion[]>();
    const rank = vi.spyOn(mailClient, "rankContactGroupCandidates").mockReturnValueOnce(old.promise).mockResolvedValue([]);
    render(<ContactMemberPicker group={group} profiles={profiles} onAdd={vi.fn()} onClose={() => {}} />);
    await tick();
    fireEvent.click(list().getByRole("option", { name: "Carol" }));
    await act(async () => old.resolve([{ id: "bob", relatedTo: ["alice@example.com"] }]));
    expect(list().getAllByRole("option")[0]).toHaveAccessibleName("Carol, selected");
    await tick(); expect(rank).toHaveBeenLastCalledWith("team", ["carol"], []);
    fireEvent.click(screen.getByRole("button", { name: "Unselect Carol" }));
    await tick(); expect(rank).toHaveBeenLastCalledWith("team", [], []);
  });

  it("keeps ordinary search and typed addresses when ranking fails", async () => {
    vi.spyOn(mailClient, "rankContactGroupCandidates").mockRejectedValue(new Error("offline index"));
    vi.spyOn(console, "error").mockImplementation(() => {});
    const add = vi.fn();
    render(<ContactMemberPicker group={group} profiles={profiles} onAdd={add} onClose={() => {}} />);
    await tick();
    const input=screen.getByRole("combobox", { name: "Find contacts to add" });
    fireEvent.change(input,{target:{value:"new@example.com"}});
    fireEvent.keyDown(input,{key:"Enter"}); await tick();
    expect(mailClient.rankContactGroupCandidates).toHaveBeenLastCalledWith("team", [], ["new@example.com"]);
    fireEvent.click(screen.getByRole("button", { name: "Add 1 Member" }));
    expect(add).toHaveBeenCalledWith([], ["new@example.com"]);
  });
});
