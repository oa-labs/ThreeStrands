import { describe, expect, it } from "vitest";
import { parseAddress, simplifyDisplayName, splitAddressList } from "./emailAddress";

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

describe("simplifyDisplayName", () => {
  it("uses the first word of an ordinary display name", () => {
    expect(simplifyDisplayName("Jane Doe")).toBe("Jane");
  });

  it("removes commas from the simplified display name", () => {
    expect(simplifyDisplayName("Bates, Dan")).toBe("Bates");
    expect(simplifyDisplayName("'The Dev Shop, LLC' via CS-PMO")).toBe("The Dev Shop LLC");
  });

  it("treats a leading single-quoted name as one word and removes its quotes", () => {
    expect(simplifyDisplayName("'The Dev Shop LLC' via CS-PMO")).toBe("The Dev Shop LLC");
  });

  it("does not mistake an apostrophe within a name for wrapping quotes", () => {
    expect(simplifyDisplayName("Conan O'Brien")).toBe("Conan");
  });
});

describe("splitAddressList", () => {
  it("does not split on a comma inside a quoted display name", () => {
    expect(splitAddressList('"Bates, Daniel R" <daniel@example.com>, bethgold@gmail.com')).toEqual([
      '"Bates, Daniel R" <daniel@example.com>',
      "bethgold@gmail.com",
    ]);
  });
});
