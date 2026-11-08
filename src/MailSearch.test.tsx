import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MailSearch } from "./MailSearch";
import { readSavedSearches, SAVED_SEARCHES_KEY } from "./savedSearches";

function renderSearch(overrides: Partial<Parameters<typeof MailSearch>[0]> = {}) {
  const props = { inputRef: createRef<HTMLInputElement>(), query: "invoice", onCommit: vi.fn(), onInput: vi.fn(), onEscape: vi.fn(), inboxOnly: true, onToggleInboxOnly: vi.fn(), accounts: [{ email: "work@example.com" }, { email: "personal@example.com" }], accountId: "work@example.com", onManageViews: vi.fn(), ...overrides };
  return { ...render(<MailSearch {...props} />), props };
}
afterEach(() => { cleanup(); localStorage.removeItem(SAVED_SEARCHES_KEY); vi.restoreAllMocks(); });
describe("named mail searches", () => {
  it("saves the latest typed query with a name and explicitly selected scope", () => {
    const { props } = renderSearch({ activeFilters: new Set(["starred"]) });
    fireEvent.change(screen.getByRole("textbox", { name: "Search Mail" }), { target: { value: "from:ana has:attachment" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Search" }));
    const dialog = screen.getByRole("dialog", { name: "Save Search" });
    expect(within(dialog).getByRole("textbox", { name: "Search" })).toHaveValue("from:ana has:attachment");
    expect(within(dialog).getByRole("combobox", { name: "Account scope" })).toHaveValue("work@example.com");
    expect(within(dialog).getByRole("combobox")).toHaveAccessibleDescription();
    expect(within(dialog).getByRole("button", { name: "Save Search" })).toBeDisabled();
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "Files from Ana" } });
    fireEvent.change(within(dialog).getByRole("combobox"), { target: { value: "personal@example.com" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Search" }));
    expect(props.onCommit).toHaveBeenCalledWith("from:ana has:attachment");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(readSavedSearches()).toEqual([expect.objectContaining({ name: "Files from Ana", query: "from:ana has:attachment", accountId: "personal@example.com", inboxOnly: true, messageFilters: ["starred"] })]);
  });
  it("opens the shared Split Inboxes management rather than a separate catalog", () => {
    const { props } = renderSearch();
    fireEvent.click(screen.getByRole("button", { name: "Saved searches" }));
    expect(props.onManageViews).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("keeps the entered name on storage failure and allows retry", () => {
    renderSearch();
    fireEvent.click(screen.getByRole("button", { name: "Save Search" }));
    const dialog = screen.getByRole("dialog", { name: "Save Search" });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), { target: { value: "Invoices" } });
    const write = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("Storage full"); });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Search" }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent("Storage full");
    expect(within(dialog).getByRole("textbox", { name: "Name" })).toHaveValue("Invoices");
    write.mockRestore();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save Search" }));
    expect(readSavedSearches()).toHaveLength(1);
  });
  it("cancels without saving and hides Save Search for an empty query", () => {
    renderSearch({ query: "" });
    expect(screen.queryByRole("button", { name: "Save Search" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Search Mail" }), { target: { value: "invoice" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Search" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(readSavedSearches()).toEqual([]);
  });
});
