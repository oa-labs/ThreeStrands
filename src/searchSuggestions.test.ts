import { describe, expect, it } from "vitest";
import {
  applySuggestion,
  contactQueryFor,
  formatOperatorValue,
  formatSearchDate,
  MAX_SEARCH_SUGGESTIONS,
  searchSuggestions,
  tokenAt,
  type SuggestionSources,
} from "./searchSuggestions";

const sources = (overrides: Partial<SuggestionSources> = {}): SuggestionSources => ({
  labels: [],
  contacts: [],
  recentSearches: [],
  now: new Date(2026, 9, 16),
  ...overrides,
});

function suggest(query: string, overrides: Partial<SuggestionSources> = {}, caret = query.length) {
  return searchSuggestions(query, tokenAt(query, caret), sources(overrides));
}

describe("tokenAt", () => {
  it("finds the operator and value under the caret", () => {
    expect(tokenAt("roadmap from:ali", 16)).toEqual({ start: 8, end: 16, operator: "from", value: "ali" });
    expect(tokenAt("FROM:Ali", 8)).toEqual({ start: 0, end: 8, operator: "from", value: "Ali" });
  });

  it("treats a word in the middle of the query as the token when the caret is inside it", () => {
    expect(tokenAt("from:ali roadmap", 3)).toEqual({ start: 0, end: 8, operator: "from", value: "ali" });
  });

  it("keeps a quoted value with spaces as one token", () => {
    expect(tokenAt("label:\"My Lab", 13)).toEqual({ start: 0, end: 13, operator: "label", value: "My Lab" });
  });

  it("returns an empty bare token after a space", () => {
    expect(tokenAt("from:ali ", 9)).toEqual({ start: 9, end: 9, operator: null, value: "" });
  });

  it("leaves unknown operators as bare words", () => {
    expect(tokenAt("category:promotions", 19).operator).toBeNull();
  });
});

describe("searchSuggestions", () => {
  it("offers recent searches only when the box is empty", () => {
    expect(suggest("", { recentSearches: ["from:ana", "invoice"] }).map((item) => item.label)).toEqual(["from:ana", "invoice"]);
    expect(suggest("in", { recentSearches: ["invoice"] }).some((item) => item.kind === "recent")).toBe(false);
  });

  it("completes operator names from a bare word", () => {
    expect(suggest("f").map((item) => item.label)).toEqual(["from:"]);
    expect(suggest("is").map((item) => item.label)).toEqual(["is:unread", "is:starred"]);
    expect(suggest("roadmap")).toEqual([]);
  });

  it("offers matching contacts after from: and to:, inserting the address", () => {
    const contacts = [{ email: "ana@example.com", name: "Ana Diaz" }, { email: "bo@example.com", name: null }];
    const items = suggest("to:a", { contacts });
    expect(items.map((item) => [item.label, item.detail, item.replacement])).toEqual([
      ["Ana Diaz", "ana@example.com", "to:ana@example.com"],
      ["bo@example.com", undefined, "to:bo@example.com"],
    ]);
    expect(contactQueryFor(tokenAt("to:a", 4))).toBe("a");
    expect(contactQueryFor(tokenAt("subject:a", 9))).toBeNull();
  });

  it("offers user labels after label:, quoting names with spaces", () => {
    const items = suggest("label:pro", { labels: ["Projects", "My Projects", "Receipts", "projects"] });
    expect(items.map((item) => item.replacement)).toEqual(["label:Projects", "label:\"My Projects\""]);
  });

  it("offers system locations and labels after in:", () => {
    expect(suggest("in:").map((item) => item.label)).toEqual(["inbox", "sent", "archive", "trash", "spam", "anywhere"]);
    expect(suggest("in:t", { labels: ["Travel"] }).map((item) => item.replacement)).toEqual(["in:trash", "in:Travel"]);
  });

  it("offers state values after is: and has:", () => {
    expect(suggest("is:s").map((item) => item.replacement)).toEqual(["is:starred"]);
    expect(suggest("has:").map((item) => item.replacement)).toEqual(["has:attachment"]);
  });

  it("offers dated presets after after: and before: in the parser's date form", () => {
    expect(suggest("after:").map((item) => [item.label, item.replacement])).toEqual([
      ["Past week", "after:2026/10/09"],
      ["Past month", "after:2026/09/16"],
      ["Past 3 months", "after:2026/07/16"],
      ["Past year", "after:2025/10/16"],
    ]);
    expect(suggest("before:").map((item) => item.label)[0]).toBe("Older than a week");
  });

  it("pins month presets to the end of a shorter month", () => {
    const items = suggest("after:", { now: new Date(2026, 2, 31) });
    expect(items.find((item) => item.label === "Past month")?.replacement).toBe("after:2026/02/28");
    const leap = suggest("after:", { now: new Date(2028, 1, 29) });
    expect(leap.find((item) => item.label === "Past year")?.replacement).toBe("after:2027/02/28");
  });

  it("does not suggest back a value already typed in full", () => {
    expect(suggest("is:unread")).toEqual([]);
  });

  it("caps the list", () => {
    const labels = Array.from({ length: 20 }, (_, index) => `Label ${index}`);
    expect(suggest("label:", { labels })).toHaveLength(MAX_SEARCH_SUGGESTIONS);
  });
});

describe("applySuggestion", () => {
  it("replaces only the token and adds a space after a finished term", () => {
    const query = "roadmap from:an";
    const [ana] = suggest(query, { contacts: [{ email: "ana@example.com", name: "Ana" }] });
    expect(applySuggestion(query, tokenAt(query, query.length), ana)).toEqual({
      query: "roadmap from:ana@example.com ",
      caret: 29,
    });
  });

  it("leaves the caret after the colon for an operator name", () => {
    const [from] = suggest("fr");
    expect(applySuggestion("fr", tokenAt("fr", 2), from)).toEqual({ query: "from:", caret: 5 });
  });

  it("keeps the text after a token edited in the middle", () => {
    const query = "is:u roadmap";
    const [unread] = suggest(query, {}, 4);
    expect(applySuggestion(query, tokenAt(query, 4), unread)).toEqual({ query: "is:unread roadmap", caret: 10 });
  });

  it("replaces the whole query with a recent search", () => {
    const [recent] = suggest("", { recentSearches: ["from:ana invoice"] });
    expect(applySuggestion("", tokenAt("", 0), recent)).toEqual({ query: "from:ana invoice", caret: 16 });
  });
});

describe("formatting", () => {
  it("quotes values with whitespace and drops stray quotes", () => {
    expect(formatOperatorValue("Work")).toBe("Work");
    expect(formatOperatorValue("Clients/Acme Corp")).toBe("\"Clients/Acme Corp\"");
    expect(formatOperatorValue("a \"b\"")).toBe("\"a b\"");
  });

  it("formats dates as YYYY/MM/DD", () => {
    expect(formatSearchDate(new Date(2026, 0, 5))).toBe("2026/01/05");
  });
});
