import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FindOrCreatePicker } from "./FindOrCreatePicker";

afterEach(cleanup);

const items = [{ id: "a", name: "Alpha" }, { id: "b", name: "Beta" }];

function renderPicker(onSelect = vi.fn(), onCreate = vi.fn(), extra: Partial<Parameters<typeof FindOrCreatePicker<(typeof items)[number]>>[0]> = {}) {
  render(
    <FindOrCreatePicker
      items={items}
      getSearchText={(item) => item.name}
      placeholder="Find or create"
      ariaLabel="Find or Create an Item"
      listId="items"
      listLabel="Items"
      emptyMessage="No items"
      createLabel={(name) => `Create ${name}`}
      onSelect={onSelect}
      onCreate={onCreate}
      renderItem={(item, option) => (
        <div key={item.id} id={option.id} role="option" aria-selected={option.active}
          className={option.active ? "highlighted" : undefined} onMouseEnter={option.onMouseEnter} onClick={option.onClick}>
          {item.name}
        </div>
      )}
      {...extra}
    />,
  );
  return { onSelect, onCreate };
}

describe("FindOrCreatePicker", () => {
  it("filters options and selects the active row with the keyboard", () => {
    const { onSelect, onCreate } = renderPicker();
    const input = screen.getByRole("combobox", { name: "Find or Create an Item" });
    // macOS inline predictions would swallow the first Escape in this keyboard-driven field.
    expect(input).toHaveAttribute("writingsuggestions", "false");
    fireEvent.change(input, { target: { value: "beta" } });

    expect(screen.getByRole("listbox", { name: "Items" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Beta" })).toHaveAttribute("id", "items-option-0");
    expect(screen.queryByRole("option", { name: "Alpha" })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Create beta" })).not.toBeInTheDocument();

    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith(items[1], "beta");
    expect(onCreate).not.toHaveBeenCalled();
  });

  it("creates a non-matching query and exposes it as the active descendant", () => {
    const { onCreate, onSelect } = renderPicker();
    const input = screen.getByRole("combobox", { name: "Find or Create an Item" });
    fireEvent.change(input, { target: { value: "Gamma" } });

    expect(input).toHaveAttribute("aria-activedescendant", "items-option-0");
    const create = screen.getByRole("option", { name: "Create Gamma" });
    expect(create).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onCreate).toHaveBeenCalledWith("Gamma");
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("moves the active option with the arrow keys", () => {
    renderPicker();
    const input = screen.getByRole("combobox", { name: "Find or Create an Item" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(input).toHaveAttribute("aria-activedescendant", "items-option-1");
    expect(screen.getByRole("option", { name: "Beta" })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(input).toHaveAttribute("aria-activedescendant", "items-option-0");
  });

  it("starts on the item a query names exactly and offers no create row for it", () => {
    renderPicker(vi.fn(), vi.fn(), { isExactMatch: (item, query) => `${item.id}@example.com` === query.toLowerCase() });
    const input = screen.getByRole("combobox", { name: "Find or Create an Item" });
    // The exact match isn't a substring of its search text, and still outranks list order.
    fireEvent.change(input, { target: { value: "B@example.com" } });
    expect(screen.getByRole("option", { name: "Beta" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("option", { name: /Create/ })).not.toBeInTheDocument();
  });

  it("shows a notice instead of the create row when the query can't be created", () => {
    const { onCreate } = renderPicker(vi.fn(), vi.fn(), { notice: (query) => query === "Taken" ? "Taken is already in use" : null });
    const input = screen.getByRole("combobox", { name: "Find or Create an Item" });
    fireEvent.change(input, { target: { value: "Taken" } });
    expect(screen.getByRole("status")).toHaveTextContent("Taken is already in use");
    expect(screen.queryByRole("option", { name: "Create Taken" })).not.toBeInTheDocument();
    expect(screen.queryByText("No items")).not.toBeInTheDocument();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onCreate).not.toHaveBeenCalled();
  });

  it("clears the query after a pick unless the create is rejected", () => {
    const onCreate = vi.fn((query: string) => query !== "bad");
    renderPicker(vi.fn(), onCreate, { clearOnPick: true });
    const input = screen.getByRole("combobox", { name: "Find or Create an Item" });
    fireEvent.change(input, { target: { value: "alp" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).toHaveValue("");
    fireEvent.change(input, { target: { value: "bad" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input).toHaveValue("bad");
    fireEvent.change(input, { target: { value: "Gamma" } });
    fireEvent.click(screen.getByRole("option", { name: "Create Gamma" }));
    expect(input).toHaveValue("");
  });

  it("keeps tokens inside the field and removes the last one with Backspace in an empty field", () => {
    const onRemoveLastToken = vi.fn();
    renderPicker(vi.fn(), vi.fn(), { tokens: <span role="group" aria-label="Chosen">Alpha</span>, onRemoveLastToken });
    const input = screen.getByRole("combobox", { name: "Find or Create an Item" });
    expect(input.parentElement).toContainElement(screen.getByRole("group", { name: "Chosen" }));
    fireEvent.change(input, { target: { value: "x" } });
    fireEvent.keyDown(input, { key: "Backspace" });
    expect(onRemoveLastToken).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "" } });
    fireEvent.keyDown(input, { key: "Backspace" });
    expect(onRemoveLastToken).toHaveBeenCalledTimes(1);
  });
});
