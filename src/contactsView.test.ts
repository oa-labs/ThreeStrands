import { afterEach, describe, expect, it } from "vitest";
import { adjacentContactsView, readContactsView, writeContactsView } from "./contactsView";

describe("contactsView", () => {
  afterEach(() => localStorage.removeItem("threestrands.contacts.view"));

  it("remembers the chosen view and ignores unknown stored values", () => {
    expect(readContactsView()).toBe("all");
    writeContactsView("keepInTouch");
    expect(readContactsView()).toBe("keepInTouch");
    localStorage.setItem("threestrands.contacts.view", "favorites");
    expect(readContactsView()).toBe("all");
  });

  it("alternates between the two views in either direction", () => {
    expect(adjacentContactsView("all", 1)).toBe("keepInTouch");
    expect(adjacentContactsView("keepInTouch", 1)).toBe("all");
    expect(adjacentContactsView("all", -1)).toBe("keepInTouch");
    expect(adjacentContactsView("keepInTouch", -1)).toBe("all");
  });
});
