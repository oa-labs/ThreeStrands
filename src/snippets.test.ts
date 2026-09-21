import { describe, expect, it } from "vitest";
import { firstNameFromRecipient, renderSnippetBody } from "./snippets";

describe("renderSnippetBody", () => {
  it("replaces {first_name} with the resolved value", () => {
    expect(renderSnippetBody("Hi {first_name}, thanks!", { firstName: "Jane" })).toBe("Hi Jane, thanks!");
  });

  it("leaves the placeholder untouched when no value is available", () => {
    expect(renderSnippetBody("Hi {first_name}!", {})).toBe("Hi {first_name}!");
  });

  it("leaves an unknown token untouched rather than stripping it", () => {
    expect(renderSnippetBody("Hi {last_name}!", { firstName: "Jane" })).toBe("Hi {last_name}!");
  });

  it("replaces every occurrence of a token", () => {
    expect(renderSnippetBody("{first_name}, hi {first_name}", { firstName: "Jane" })).toBe("Jane, hi Jane");
  });
});

describe("firstNameFromRecipient", () => {
  it("extracts the first name from a display name", () => {
    expect(firstNameFromRecipient("Jane Doe <jane@example.com>")).toBe("Jane");
  });

  it("uses only the first of multiple recipients", () => {
    expect(firstNameFromRecipient("Jane Doe <jane@example.com>, Bob Smith <bob@example.com>")).toBe("Jane");
  });

  it("returns undefined when there is no display name", () => {
    expect(firstNameFromRecipient("jane@example.com")).toBeUndefined();
  });

  it("returns undefined for an empty recipient field", () => {
    expect(firstNameFromRecipient("")).toBeUndefined();
  });
});
