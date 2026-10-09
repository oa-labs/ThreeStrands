import { describe, expect, it } from "vitest";
import { plural } from "./plural";

describe("plural", () => {
  it("uses the singular only for exactly one", () => {
    expect(plural(1, "change")).toBe("1 change");
    expect(plural(0, "change")).toBe("0 changes");
    expect(plural(2, "change")).toBe("2 changes");
  });

  it("accepts an irregular plural", () => {
    expect(plural(1, "person", "people")).toBe("1 person");
    expect(plural(3, "person", "people")).toBe("3 people");
  });
});
