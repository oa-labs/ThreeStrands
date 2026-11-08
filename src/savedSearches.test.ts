import { afterEach, describe, expect, it, vi } from "vitest";
import { deleteSavedSearch, readSavedSearches, saveSearch, SAVED_SEARCHES_KEY } from "./savedSearches";

const search = { name: "Invoices", query: 'has:attachment subject:invoice', accountId: "work@example.com", inboxOnly: true };
afterEach(() => { localStorage.removeItem(SAVED_SEARCHES_KEY); vi.restoreAllMocks(); });
describe("saved searches", () => {
  it("persists the query, name, explicit scope and Inbox Only across reads", () => {
    const [saved] = saveSearch({ ...search, name: " Invoices ", query: ` ${search.query} ` });
    expect(readSavedSearches()).toEqual([{ ...search, id: saved.id, messageFilters: [] }]);
    expect(deleteSavedSearch(saved.id)).toEqual([]);
    expect(readSavedSearches()).toEqual([]);
  });
  it("allows the same name in distinct scopes but rejects duplicates within a scope", () => {
    saveSearch({ ...search, messageFilters: ["unread", "starred"] });
    expect(readSavedSearches()[0].messageFilters).toEqual(["unread", "starred"]);
    saveSearch({ ...search, accountId: null });
    expect(() => saveSearch({ ...search, name: " invoices " })).toThrow(/already exists/);
    expect(readSavedSearches()).toHaveLength(2);
  });
  it.each(["{broken", '{"version":2,"searches":[]}', '{"version":1,"searches":[{"query":"hi"}]}'])("preserves unreadable data instead of overwriting it", (raw) => {
    localStorage.setItem(SAVED_SEARCHES_KEY, raw);
    expect(() => saveSearch(search)).toThrow();
    expect(() => deleteSavedSearch("missing")).toThrow();
    expect(localStorage.getItem(SAVED_SEARCHES_KEY)).toBe(raw);
  });
  it("reports write failure without reporting an unsaved search as saved", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("Storage full"); });
    expect(() => saveSearch(search)).toThrow("Storage full");
    expect(readSavedSearches()).toEqual([]);
  });
  it.each([{ name: " " }, { query: " " }, { accountId: "" }])("rejects incomplete search definitions", (invalid) => {
    expect(() => saveSearch({ ...search, ...invalid })).toThrow();
  });
});

describe("editing saved searches", () => {
  it("preserves identity and other saved searches when editing conditions and scope", async () => {
    const { updateSavedSearch } = await import("./savedSearches");
    const [first] = saveSearch(search);
    saveSearch({ ...search, name: "Other" });
    const next = updateSavedSearch(first.id, { name: "All bills", query: "has:attachment", accountId: null, inboxOnly: false, messageFilters: ["starred"] });
    expect(next[0]).toEqual({ id: first.id, name: "All bills", query: "has:attachment", accountId: null, inboxOnly: false, messageFilters: ["starred"] });
    expect(next[1].name).toBe("Other");
    expect(readSavedSearches()).toEqual(next);
  });
  it("retains the old definition on duplicate names or write failure", async () => {
    const { updateSavedSearch } = await import("./savedSearches");
    const [first] = saveSearch(search);
    saveSearch({ ...search, name: "Other" });
    const before = readSavedSearches();
    expect(() => updateSavedSearch(first.id, { name: "other" })).toThrow(/already exists/);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("Storage full"); });
    expect(() => updateSavedSearch(first.id, { query: "new" })).toThrow("Storage full");
    expect(readSavedSearches()).toEqual(before);
  });
});
