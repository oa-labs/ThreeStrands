import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CONTACT_SUGGESTION_DEBOUNCE_MS, SEARCH_DEBOUNCE_MS, SearchField } from "./SearchField";
import { RECENT_SEARCHES_KEY } from "./recentSearches";

function renderField(query = "", overrides: Partial<Parameters<typeof SearchField>[0]> = {}) {
  const props = {
    inputRef: createRef<HTMLInputElement>(),
    query,
    onCommit: vi.fn(),
    onInput: vi.fn(),
    onEscape: vi.fn(),
    inboxOnly: false,
    onToggleInboxOnly: vi.fn(),
    ...overrides,
  };
  const view = render(<SearchField {...props} />);
  return { ...view, props, input: screen.getByRole("textbox", { name: "Search Mail" }) as HTMLInputElement };
}

describe("SearchField", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    localStorage.removeItem(RECENT_SEARCHES_KEY);
  });

  it("commits once after typing pauses, not on every keystroke", () => {
    const { input, props } = renderField();
    fireEvent.change(input, { target: { value: "r" } });
    fireEvent.change(input, { target: { value: "ro" } });
    fireEvent.change(input, { target: { value: "roa" } });
    expect(input).toHaveValue("roa");
    expect(props.onInput).toHaveBeenCalledTimes(3);

    act(() => { vi.advanceTimersByTime(SEARCH_DEBOUNCE_MS - 1); });
    expect(props.onCommit).not.toHaveBeenCalled();
    act(() => { vi.advanceTimersByTime(1); });
    expect(props.onCommit).toHaveBeenCalledTimes(1);
    expect(props.onCommit).toHaveBeenCalledWith("roa");
  });

  it("commits a cleared field immediately", () => {
    const { input, props } = renderField("roadmap");
    fireEvent.change(input, { target: { value: "" } });
    act(() => { vi.advanceTimersByTime(0); });
    expect(props.onCommit).toHaveBeenCalledWith("");
  });

  it("adopts a query cleared elsewhere", () => {
    const { input, props, rerender } = renderField("roadmap");
    expect(input).toHaveValue("roadmap");
    rerender(<SearchField {...props} query="" />);
    expect(input).toHaveValue("");
    act(() => { vi.advanceTimersByTime(SEARCH_DEBOUNCE_MS); });
    expect(props.onCommit).not.toHaveBeenCalled();
  });

  it("clears and closes on Escape without committing a stale draft", () => {
    const { input, props } = renderField();
    fireEvent.change(input, { target: { value: "roa" } });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(input).toHaveValue("");
    expect(props.onEscape).toHaveBeenCalledTimes(1);
    act(() => { vi.advanceTimersByTime(SEARCH_DEBOUNCE_MS); });
    expect(props.onCommit).not.toHaveBeenCalled();
  });

  it("closes from the visible button without committing a pending draft", () => {
    const { input, props } = renderField("Welcome");
    fireEvent.change(input, { target: { value: "roadmap" } });
    fireEvent.click(screen.getByRole("button", { name: "Close Search" }));
    expect(input).toHaveValue("");
    expect(props.onEscape).toHaveBeenCalledOnce();
    act(() => { vi.advanceTimersByTime(SEARCH_DEBOUNCE_MS); });
    expect(props.onCommit).not.toHaveBeenCalled();
  });

  it("turns off inline predictions so a single Escape reaches the field", () => {
    const { input } = renderField();
    expect(input).toHaveAttribute("writingsuggestions", "false");
    expect(input).toHaveAttribute("autocorrect", "off");
    expect(input).toHaveAttribute("autocomplete", "off");
    expect(input).toHaveAttribute("spellcheck", "false");
  });

  it("offers the Inbox Only filter as soon as there is text to search", () => {
    const { input, props } = renderField();
    expect(screen.queryByRole("button", { name: "Search only the Inbox" })).not.toBeInTheDocument();
    fireEvent.change(input, { target: { value: "r" } });
    const toggle = screen.getByRole("button", { name: "Search only the Inbox" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(toggle).not.toHaveTextContent("Inbox Only");
    fireEvent.click(toggle);
    expect(props.onToggleInboxOnly).toHaveBeenCalledTimes(1);
  });

  it("shows the compact Inbox Only filter as pressed while the search is narrowed", () => {
    renderField("roadmap", { inboxOnly: true });
    const toggle = screen.getByRole("button", { name: "Search only the Inbox" });
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(toggle).not.toHaveTextContent("Inbox Only");
  });

  it("shows the / hint only while the field is idle", () => {
    const { input } = renderField();
    const box = input.closest(".search-box")!;
    expect(box).toHaveClass("idle");
    expect(box.querySelector("kbd")).toHaveTextContent("/");
    act(() => input.focus());
    expect(box).not.toHaveClass("idle");
    expect(box.querySelector("kbd")).toBeNull();
    act(() => input.blur());
    fireEvent.change(input, { target: { value: "roadmap" } });
    expect(box).not.toHaveClass("idle");
    expect(box.querySelector("kbd")).toBeNull();
  });
});

describe("SearchField suggestions", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    localStorage.removeItem(RECENT_SEARCHES_KEY);
  });

  function type(input: HTMLInputElement, value: string) {
    act(() => input.focus());
    fireEvent.change(input, { target: { value } });
  }

  const options = () => within(screen.getByRole("listbox")).getAllByRole("option").map((option) => option.textContent);

  it("completes an operator name and keeps going into its values", () => {
    const { input } = renderField();
    type(input, "i");
    expect(options()).toEqual(["in:Folder or location", "is:unreadUnread", "is:starredStarred"]);
    // A bare word waits for an arrow key before Enter picks anything.
    expect(input).not.toHaveAttribute("aria-activedescendant");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(screen.getByRole("option", { name: /is:starred/ })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByRole("option", { name: /in:/ })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).toHaveValue("in:");
    expect(options()[0]).toBe("inboxInbox");
    // An operator's first value is ready for Enter.
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).toHaveValue("in:inbox ");
  });

  it("looks up contacts after from: and inserts the chosen address", async () => {
    const loadContacts = vi.fn(async (query: string) => [{ email: `${query}na@example.com`, name: "Ana Diaz" }]);
    const { input, props } = renderField("", { loadContacts });
    type(input, "roadmap from:a");
    expect(loadContacts).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(CONTACT_SUGGESTION_DEBOUNCE_MS); });
    expect(loadContacts).toHaveBeenCalledWith("a");
    fireEvent.click(screen.getByRole("option", { name: /Ana Diaz/ }));
    expect(input).toHaveValue("roadmap from:ana@example.com ");
    act(() => { vi.advanceTimersByTime(SEARCH_DEBOUNCE_MS); });
    expect(props.onCommit).toHaveBeenLastCalledWith("roadmap from:ana@example.com ");
  });

  it("offers labels after label:", () => {
    const { input } = renderField("", { labels: ["Receipts", "Travel Plans"] });
    type(input, "label:tr");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).toHaveValue("label:\"Travel Plans\" ");
  });

  it("closes the list on the first Escape and clears the search on the second", () => {
    const { input, props } = renderField();
    type(input, "is:");
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(input).toHaveValue("is:");
    expect(props.onEscape).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Escape" });
    expect(input).toHaveValue("");
    expect(props.onEscape).toHaveBeenCalledTimes(1);
  });

  it("remembers submitted searches and offers them in an empty box", () => {
    const { input, props } = renderField();
    type(input, "invoice");
    fireEvent.keyDown(input, { key: "Enter" });
    // Enter runs the search without waiting for the typing pause.
    expect(props.onCommit).toHaveBeenCalledWith("invoice");
    fireEvent.change(input, { target: { value: "" } });
    act(() => { vi.advanceTimersByTime(0); });
    expect(screen.getByRole("listbox", { name: "Recent searches" })).toBeInTheDocument();
    expect(options()).toEqual(["invoice"]);

    fireEvent.click(screen.getByRole("option", { name: "invoice" }));
    expect(input).toHaveValue("invoice");
    expect(props.onCommit).toHaveBeenLastCalledWith("invoice");

    fireEvent.change(input, { target: { value: "from:bo" } });
    act(() => { vi.advanceTimersByTime(SEARCH_DEBOUNCE_MS); });
    act(() => input.blur());
    expect(JSON.parse(localStorage.getItem(RECENT_SEARCHES_KEY) ?? "[]")).toEqual(["invoice"]);
  });

  it("shows at most five existing recent searches under a heading without adding it to keyboard options", () => {
    localStorage.setItem(RECENT_SEARCHES_KEY, JSON.stringify(["one", "two", "three", "four", "five", "six", "seven", "eight"]));
    const { input } = renderField();
    act(() => input.focus());
    expect(screen.getByRole("heading", { name: "Recent searches" })).toBeInTheDocument();
    expect(options()).toEqual(["one", "two", "three", "four", "five"]);
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getByRole("option", { name: "one" })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(screen.getByRole("option", { name: "five" })).toHaveAttribute("aria-selected", "true");
  });

  it("refreshes history recorded by opening results when the input regains focus", () => {
    const { input } = renderField();
    act(() => input.focus());
    act(() => input.blur());
    localStorage.setItem(RECENT_SEARCHES_KEY, JSON.stringify(["opened result"]));
    act(() => input.focus());
    expect(options()).toEqual(["opened result"]);
  });

  it("does not remember a draft that was never searched", () => {
    const { input } = renderField();
    type(input, "half-typ");
    act(() => input.blur());
    expect(localStorage.getItem(RECENT_SEARCHES_KEY)).toBeNull();
  });
});
