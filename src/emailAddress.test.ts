import { describe, expect, it } from "vitest";
import { formatDisplayName, parseAddress, splitAddressList } from "./emailAddress";

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
});
