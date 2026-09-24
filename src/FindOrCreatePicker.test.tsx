import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FindOrCreatePicker } from "./FindOrCreatePicker";

afterEach(cleanup);

const items = [{ id: "a", name: "Alpha" }, { id: "b", name: "Beta" }];

function renderPicker(onSelect = vi.fn(), onCreate = vi.fn()) {
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
    />,
  );
  return { onSelect, onCreate };
}

describe("FindOrCreatePicker", () => {
  it("filters options and selects the active row with the keyboard", () => {
    const { onSelect, onCreate } = renderPicker();
    const input = screen.getByRole("combobox", { name: "Find or Create an Item" });
    fireEvent.change(input, { target: { value: "beta" } });

    expect(screen.getByRole("listbox", { name: "Items" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Beta" })).toHaveAttribute("id", "items-option-0");
    expect(screen.queryByRole("option", { name: "Alpha" })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Create beta" })).not.toBeInTheDocument();

    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith(items[1]);
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
});
