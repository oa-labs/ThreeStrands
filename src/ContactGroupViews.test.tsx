import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ContactGroupDetail, ContactMemberPicker } from "./ContactGroupViews";
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

describe("group name editing", () => {
  it("blurs on Enter and saves the edited name exactly once", async () => {
    const rename = vi.fn().mockResolvedValue(true);
    render(<ContactGroupDetail group={group} profiles={profiles} busy={false}
      onOpenContact={vi.fn()} onRename={rename} onDelete={vi.fn()}
      onAddMembers={vi.fn()} onRemoveMember={vi.fn()} />);
    const name = screen.getByRole("textbox", { name: "Group Name" });
    await act(async () => name.focus());
    expect(name).toHaveFocus();
    fireEvent.change(name, { target: { value: "Directors" } });
    expect(name).toHaveValue("Directors");
    expect(rename).not.toHaveBeenCalled();

    await act(async () => { fireEvent.keyDown(name, { key: "Enter" }); });

    expect(name).not.toHaveFocus();
    expect(rename).toHaveBeenCalledExactlyOnceWith("Directors");
  });
});

describe("group member recommendations", () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });
  const list = () => within(screen.getByRole("listbox", { name: "Contacts" }));

  it("ranks recommendations without selecting them or changing primary addresses", async () => {
    const rank = vi.spyOn(mailClient, "rankContactGroupCandidates").mockResolvedValue([{ id: "bob", relatedTo: ["alice@example.com"] }]);
    const add = vi.fn().mockResolvedValue(undefined);
    render(<ContactMemberPicker group={group} profiles={profiles} onAdd={add} onClose={() => {}} />);
    await tick();
    expect(screen.getByRole("combobox", { name: "Find Contacts to Add" })).toHaveAccessibleDescription();
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
    const input=screen.getByRole("combobox", { name: "Find Contacts to Add" });
    fireEvent.change(input,{target:{value:"new@example.com"}});
    fireEvent.keyDown(input,{key:"Enter"}); await tick();
    expect(mailClient.rankContactGroupCandidates).toHaveBeenLastCalledWith("team", [], ["new@example.com"]);
    fireEvent.click(screen.getByRole("button", { name: "Add 1 Member" }));
    expect(add).toHaveBeenCalledWith([], ["new@example.com"]);
  });
});

describe("Add Members layout and interaction", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.spyOn(mailClient, "rankContactGroupCandidates").mockResolvedValue([]);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });

  it("keeps keyboard highlight distinct from selection and reports the pending count", () => {
    const add = vi.fn();
    render(<ContactMemberPicker group={group} profiles={profiles} onAdd={add} onClose={() => {}} />);
    const input = screen.getByRole("combobox", { name: "Find Contacts to Add" });
    expect(input).toHaveFocus();
    const list = screen.getByRole("listbox", { name: "Contacts" });
    expect(list).toHaveAttribute("aria-multiselectable", "true");
    const carol = within(list).getByRole("option", { name: "Carol" });
    expect(input).toHaveAttribute("aria-activedescendant", carol.id);
    expect(carol).toHaveAttribute("aria-selected", "false");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.getByRole("combobox", { name: "Find Contacts to Add" })).toBe(input);
    expect(input).toHaveFocus();
    expect(within(list).getByRole("option", { name: "Carol, selected" })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    const bob = within(list).getByRole("option", { name: "Bob" });
    expect(input).toHaveAttribute("aria-activedescendant", bob.id);
    expect(bob).toHaveAttribute("aria-selected", "false");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.getByRole("button", { name: "Add 2 Members" })).toBeEnabled();
    expect(add).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Unselect Bob" }));
    expect(screen.getByRole("button", { name: "Add 1 Member" })).toBeEnabled();
  });

  it("keeps reasons short and makes full named evidence visible for keyboard, hover, and touch selection", async () => {
    vi.mocked(mailClient.rankContactGroupCandidates).mockResolvedValue([{ id: "bob", relatedTo: ["alice@example.com", "carol@example.com", "unknown@example.net"] }]);
    render(<ContactMemberPicker group={group} profiles={profiles} onAdd={vi.fn()} onClose={() => {}} />);
    await tick();
    const input = screen.getByRole("combobox", { name: "Find Contacts to Add" });
    const bob = screen.getByRole("option", { name: "Bob" });
    expect(bob).toHaveTextContent("Often emailed with Alice and 2 others");
    expect(bob).not.toHaveTextContent("unknown@example.net");
    // Ranking preserves Carol's keyboard highlight; moving up exposes Bob's details.
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(input).toHaveAccessibleDescription(/Alice \(alice@example.com\), Carol \(carol@example.com\), unknown@example.net/);
    const detailsId = input.getAttribute("aria-describedby")!.split(" ").at(-1)!;
    expect(document.getElementById(detailsId)).toBeVisible();
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(input).not.toHaveAccessibleDescription(/unknown@example.net/);
    fireEvent.mouseEnter(bob);
    expect(input).toHaveAccessibleDescription(/unknown@example.net/);
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.click(bob); // Touch does not require a hover first.
    await tick();
    expect(input).toHaveAccessibleDescription(/unknown@example.net/);
    expect(screen.getByRole("button", { name: "Add 1 Member" })).toBeEnabled();
  });

  it("keeps the modal focus boundary, Escape dismissal, and focus restoration", () => {
    const add = vi.fn();
    function Harness() {
      const [open, setOpen] = useState(false);
      return <><button type="button" onClick={() => setOpen(true)}>Add Members</button>
        {open ? <ContactMemberPicker group={group} profiles={profiles} onAdd={add} onClose={() => setOpen(false)} /> : null}</>;
    }
    const { container } = render(<Harness />);
    const trigger = screen.getByRole("button", { name: "Add Members" });
    act(() => trigger.focus()); fireEvent.click(trigger);
    const modal = screen.getByRole("dialog", { name: "Add Members to Team" });
    expect(modal).toHaveAttribute("data-shortcut-scope", "modal");
    expect(container).toHaveAttribute("aria-hidden", "true");
    expect(container.inert).toBe(true);
    const input = screen.getByRole("combobox", { name: "Find Contacts to Add" });
    expect(input).toHaveFocus();
    fireEvent.keyDown(input, { key: "Enter" });
    const last = screen.getByRole("button", { name: "Add 1 Member" });
    act(() => last.focus()); fireEvent.keyDown(last, { key: "Tab" });
    const close = screen.getByRole("button", { name: "Close" });
    expect(close).toHaveFocus();
    fireEvent.keyDown(close, { key: "Tab", shiftKey: true });
    expect(last).toHaveFocus();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    expect(container).not.toHaveAttribute("aria-hidden");
    expect(add).not.toHaveBeenCalled();
  });

  it("preserves the selected badges after a failed add and keeps Cancel available", async () => {
    const add = vi.fn().mockRejectedValue(new Error("Could not save members"));
    const close = vi.fn();
    render(<ContactMemberPicker group={group} profiles={profiles} onAdd={add} onClose={close} />);
    fireEvent.click(screen.getByRole("option", { name: "Bob" }));
    fireEvent.click(screen.getByRole("button", { name: "Add 1 Member" }));
    await act(async () => {});
    expect(screen.getByRole("alert")).toHaveTextContent("Could not save members");
    expect(screen.getByRole("button", { name: "Unselect Bob" })).toBeVisible();
    expect(close).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(close).toHaveBeenCalledOnce();
  });
});
