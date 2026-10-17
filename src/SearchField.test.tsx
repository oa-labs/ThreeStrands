import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SEARCH_DEBOUNCE_MS, SearchField } from "./SearchField";

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
    expect(toggle).toHaveTextContent("Inbox Only");
    fireEvent.click(toggle);
    expect(props.onToggleInboxOnly).toHaveBeenCalledTimes(1);
  });

  it("shows the Inbox Only filter as pressed while the search is narrowed", () => {
    renderField("roadmap", { inboxOnly: true });
    expect(screen.getByRole("button", { name: "Search only the Inbox" })).toHaveAttribute("aria-pressed", "true");
  });
});
