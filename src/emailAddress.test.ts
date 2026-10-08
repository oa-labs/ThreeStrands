import { describe, expect, it } from "vitest";
import { formatAddress, formatDisplayName, normalizeAddressList, parseAddress, splitAddressList } from "./emailAddress";

describe("parseAddress", () => {
  it("splits a display name and email out of a From-style header", () => {
    expect(parseAddress("Jane Doe <jane@example.com>")).toEqual({
      name: "Jane Doe",
      email: "jane@example.com",
    });
  });

  it("strips surrounding double quotes from the display name", () => {
    expect(parseAddress('"Doe, Jane" <jane@example.com>')).toEqual({
      name: "Doe, Jane",
      email: "jane@example.com",
    });
  });

  it("unescapes backslash-escaped quotes and backslashes in a quoted display name", () => {
    expect(parseAddress('"Jane \\"J\\" Doe" <jane@example.com>')).toEqual({
      name: 'Jane "J" Doe',
      email: "jane@example.com",
    });
    expect(parseAddress('"C:\\\\Docs" <jane@example.com>')).toEqual({
      name: "C:\\Docs",
      email: "jane@example.com",
    });
  });

  it("collapses runs of whitespace in the display name", () => {
    expect(parseAddress("Jane   Doe <jane@example.com>")).toEqual({
      name: "Jane Doe",
      email: "jane@example.com",
    });
  });

  it("falls back to the email when there is no display name", () => {
    expect(parseAddress("<jane@example.com>")).toEqual({
      name: "jane@example.com",
      email: "jane@example.com",
    });
    expect(parseAddress("jane@example.com")).toEqual({
      name: "jane@example.com",
      email: "jane@example.com",
    });
  });

  it("leaves a name that merely contains an apostrophe untouched", () => {
    expect(parseAddress("Conan O'Brien <conan@example.com>")).toEqual({
      name: "Conan O'Brien",
      email: "conan@example.com",
    });
  });
});

describe("formatDisplayName", () => {
  it("preserves a person's complete display name", () => {
    expect(formatDisplayName("Jane Doe")).toBe("Jane Doe");
    expect(formatDisplayName("Bates, Dan")).toBe("Bates, Dan");
  });

  it("preserves structurally different organization names", () => {
    expect(formatDisplayName("The Freedom Foundation")).toBe("The Freedom Foundation");
    expect(formatDisplayName("New York Times")).toBe("New York Times");
  });

  it("removes a delegated delivery suffix while preserving the complete name", () => {
    expect(formatDisplayName("'The Dev Shop, LLC' via CS-PMO")).toBe("The Dev Shop, LLC");
  });

  it("does not mistake apostrophes or an ordinary quoted name for delegation", () => {
    expect(formatDisplayName("Conan O'Brien")).toBe("Conan O'Brien");
    expect(formatDisplayName("'The Dev Shop, LLC'")).toBe("'The Dev Shop, LLC'");
  });
});

describe("splitAddressList", () => {
  it("does not split on a comma inside a quoted display name", () => {
    expect(splitAddressList('"Bates, Daniel R" <daniel@example.com>, bethgold@gmail.com')).toEqual([
      '"Bates, Daniel R" <daniel@example.com>',
      "bethgold@gmail.com",
    ]);
  });

  it("keeps an unquoted comma in a display name with its address", () => {
    expect(splitAddressList("Daniel O'Connor, CFA® <doconnor@wealth.example>, bethgold@gmail.com")).toEqual([
      "Daniel O'Connor, CFA® <doconnor@wealth.example>",
      "bethgold@gmail.com",
    ]);
    expect(splitAddressList("Jane Roe <jane@example.com>, Smith, Pat, PhD <pat@lab.example>").map(parseAddress)).toEqual([
      { name: "Jane Roe", email: "jane@example.com" },
      { name: "Smith, Pat, PhD", email: "pat@lab.example" },
    ]);
  });

  it("does not attach a stray name to a bare address or invent one at the end", () => {
    expect(splitAddressList("Team, bob@example.com")).toEqual(["Team", "bob@example.com"]);
    expect(splitAddressList("bob@example.com, undisclosed-recipients:;")).toEqual(["bob@example.com", "undisclosed-recipients:;"]);
  });
});

describe("formatAddress", () => {
  it("quotes a display name holding a comma or other special, escaping quotes and backslashes", () => {
    expect(formatAddress("Fischgrund, Justin", "justin@example.com")).toBe('"Fischgrund, Justin" <justin@example.com>');
    expect(formatAddress('Say "Hi" \\ there', "hi@example.com")).toBe('"Say \\"Hi\\" \\\\ there" <hi@example.com>');
    expect(formatAddress("J. Smith", "j@example.com")).toBe('"J. Smith" <j@example.com>');
    expect(formatAddress("Kelly Sjol", "kelly@example.com")).toBe("Kelly Sjol <kelly@example.com>");
  });

  it("writes the bare address without a name, or when the name is the address", () => {
    expect(formatAddress(null, "a@example.com")).toBe("a@example.com");
    expect(formatAddress("  ", "a@example.com")).toBe("a@example.com");
    expect(formatAddress("A@Example.com", "a@example.com")).toBe("a@example.com");
  });

  it("round-trips through splitting and parsing", () => {
    const names = ["Fischgrund, Justin", 'Say "Hi"', "Kelly Sjol", "O'Brien; Pat (CFO)"];
    const list = names.map((name, index) => formatAddress(name, `p${index}@example.com`)).join(", ");
    expect(splitAddressList(list).map(parseAddress)).toEqual(names.map((name, index) => ({ name, email: `p${index}@example.com` })));
  });
});

describe("normalizeAddressList", () => {
  it("repairs an unquoted comma in a name, keeping the other recipients and the trailing separator", () => {
    expect(normalizeAddressList("Fischgrund, Justin <justin@example.com>, ForceBuilders <dom@example.com>, pj@example.com, "))
      .toBe('"Fischgrund, Justin" <justin@example.com>, ForceBuilders <dom@example.com>, pj@example.com, ');
  });

  it("leaves well-formed lists, empty values, and text still being typed alone", () => {
    expect(normalizeAddressList('"Doe, Jane" <jane@example.com>, bob@example.com')).toBe('"Doe, Jane" <jane@example.com>, bob@example.com');
    expect(normalizeAddressList("")).toBe("");
    expect(normalizeAddressList("bob@example.com, jan")).toBe("bob@example.com, jan");
  });
});

