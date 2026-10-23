import { afterEach, describe, expect, it } from "vitest";
import { readLastWorkspace, readLaunchView, readLaunchWorkspace, readSideColumn, writeLastWorkspace, writeLaunchView, writeSideColumn } from "./lastWorkspace";

describe("lastWorkspace", () => {
  afterEach(() => {
    localStorage.removeItem("threestrands.workspace.last");
    localStorage.removeItem("threestrands.workspace.launch");
    localStorage.removeItem("threestrands.workspace.sideColumn");
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

  it("remembers Mail's right column apart from the view, so Mail reopens with the schedule", () => {
    expect(readSideColumn()).toBe("context");
    writeLastWorkspace("files");
    writeSideColumn("schedule");
    writeLastWorkspace(null);
    expect(readLastWorkspace()).toBeNull();
    expect(readSideColumn()).toBe("schedule");
    writeSideColumn("context");
    expect(readSideColumn()).toBe("context");
    localStorage.setItem("threestrands.workspace.sideColumn", "calendar");
    expect(readSideColumn()).toBe("context");
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
