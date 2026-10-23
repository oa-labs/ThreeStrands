import { afterEach, describe, expect, it } from "vitest";
import { readLastWorkspace, readLaunchView, readLaunchWorkspace, writeLastWorkspace, writeLaunchView } from "./lastWorkspace";

describe("lastWorkspace", () => {
  afterEach(() => {
    localStorage.removeItem("threestrands.workspace.last");
    localStorage.removeItem("threestrands.workspace.launch");
  });

  it("starts in Mail when nothing has been remembered", () => {
    expect(readLastWorkspace()).toBeNull();
  });

  it("remembers each view the navigation rail switches to", () => {
    for (const workspace of ["today", "week", "tasks", "contacts", "files"] as const) {
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

  it("launches into the last view unless Start With is set to Today", () => {
    expect(readLaunchView()).toBe("last");
    writeLastWorkspace("tasks");
    expect(readLaunchWorkspace()).toBe("tasks");

    writeLaunchView("today");
    expect(readLaunchView()).toBe("today");
    expect(readLaunchWorkspace()).toBe("today");

    writeLaunchView("last");
    expect(readLaunchWorkspace()).toBe("tasks");
  });

  it("treats an unknown Start With value as the last view", () => {
    localStorage.setItem("threestrands.workspace.launch", "calendar");
    expect(readLaunchView()).toBe("last");
  });
});
