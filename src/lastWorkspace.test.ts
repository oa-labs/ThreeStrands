import { afterEach, describe, expect, it } from "vitest";
import { readLastWorkspace, writeLastWorkspace } from "./lastWorkspace";

describe("lastWorkspace", () => {
  afterEach(() => localStorage.removeItem("threestrands.workspace.last"));

  it("starts in Mail when nothing has been remembered", () => {
    expect(readLastWorkspace()).toBeNull();
  });

  it("remembers each view the navigation rail switches to", () => {
    for (const workspace of ["week", "tasks", "contacts", "files"] as const) {
      writeLastWorkspace(workspace);
      expect(readLastWorkspace()).toBe(workspace);
    }
    writeLastWorkspace(null);
    expect(readLastWorkspace()).toBeNull();
  });

  it("reopens Mail rather than the Today calendar panel beside it", () => {
    writeLastWorkspace("files");
    writeLastWorkspace("calendar");
    expect(readLastWorkspace()).toBeNull();
  });

  it("ignores unknown stored values", () => {
    localStorage.setItem("threestrands.workspace.last", "calendar");
    expect(readLastWorkspace()).toBeNull();
    localStorage.setItem("threestrands.workspace.last", "inbox");
    expect(readLastWorkspace()).toBeNull();
  });
});
