import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RecipientField } from "./RecipientField";
import { mailClient } from "./data/client";
import type { ContactSuggestion } from "./domain";

const bob: ContactSuggestion = { email: "bob@example.com", displayName: "Bob", sentCount: 2, receivedCount: 0, lastInteractedAt: "", pinned: false, relatedTo: ["alice@example.com"] };
const props = { id: "to" as const, label: "To", value: "", account: "me@example.com", disabled: false, onChange: vi.fn() };
const context = { seeds: ["alice@example.com"], excluded: ["alice@example.com", "secret@example.com"] };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }
const tick = () => act(async () => { await vi.advanceTimersByTimeAsync(150); });

describe("contextual recipient lookup", () => {
  beforeEach(() => { vi.useFakeTimers({ shouldAdvanceTime: true }); vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([]); });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });

  it("pastes a mailto destination without importing link headers", () => {
    const change = vi.fn();
    render(<RecipientField {...props} onChange={change} />);
    fireEvent.paste(screen.getByRole("textbox", { name: "To" }), { clipboardData: { getData: () => "mailto:jane@example.com?bcc=hidden@example.com&subject=Hello" } });
    expect(change).toHaveBeenCalledWith("jane@example.com, ");
  });

  it("offers empty-input recommendations on focus and adds only the selected person", async () => {
    const lookup = vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([bob]);
    const change = vi.fn();
    render(<RecipientField {...props} context={context} onChange={change} />);
    expect(lookup).not.toHaveBeenCalled();
    fireEvent.focus(screen.getByRole("textbox", { name: "To" }));
    await tick();
    expect(lookup).toHaveBeenLastCalledWith("me@example.com", "", 8, context);
    const option = screen.getByRole("option", { name: /Bob/ });
    expect(option).toHaveTextContent("Often emailed with alice@example.com");
    expect(change).not.toHaveBeenCalled();
    fireEvent.mouseDown(option);
    expect(change).toHaveBeenCalledWith("Bob <bob@example.com>, ");
  });

  it("invalidates old context and account results immediately, including during debounce", async () => {
    const old = deferred<ContactSuggestion[]>();
    const current = deferred<ContactSuggestion[]>();
    const lookup = vi.spyOn(mailClient, "listContactSuggestions").mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    const view = render(<RecipientField {...props} context={context} />);
    const input = screen.getByRole("textbox", { name: "To" });
    act(() => input.focus()); await tick();
    const next = { seeds: ["carol@example.com"], excluded: ["carol@example.com"] };
    view.rerender(<RecipientField {...props} account="work@example.com" context={next} />);
    await act(async () => old.resolve([bob]));
    expect(screen.queryByRole("option", { name: /Bob/ })).not.toBeInTheDocument();
    await tick();
    expect(lookup).toHaveBeenLastCalledWith("work@example.com", "", 8, next);
    fireEvent.blur(input);
    await act(async () => current.resolve([bob]));
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("excludes recipients from every field and keeps Escape dismissal through a requery", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([bob, { ...bob, email: "secret@example.com", displayName: "Secret" }]);
    const view = render(<RecipientField {...props} context={context} />);
    const input = screen.getByRole("textbox", { name: "To" });
    act(() => input.focus()); await tick();
    expect(screen.queryByRole("option", { name: /Secret/ })).not.toBeInTheDocument();
    fireEvent.keyDown(input, { key: "Escape" });
    view.rerender(<RecipientField {...props} context={{ ...context, seeds: [...context.seeds, "dave@example.com"] }} />);
    await tick(); expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("falls back to ordinary search with no relationship reason after a lookup failure", async () => {
    const lookup = vi.spyOn(mailClient, "listContactSuggestions").mockRejectedValueOnce(new Error("index unavailable")).mockResolvedValue([bob]);
    vi.spyOn(console, "error").mockImplementation(() => {});
    render(<RecipientField {...props} context={context} />);
    fireEvent.change(screen.getByRole("textbox", { name: "To" }), { target: { value: "bob" } });
    await tick();
    expect(lookup).toHaveBeenLastCalledWith("me@example.com", "bob", 8);
    expect(screen.getByRole("option", { name: /Bob/ })).not.toHaveTextContent("Often emailed with");
  });
});
