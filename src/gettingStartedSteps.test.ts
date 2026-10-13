import { readdirSync } from "node:fs";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  completeSteps,
  docsUrl,
  GETTING_STARTED_STEPS,
  GETTING_STARTED_STORAGE_KEY,
  INITIAL_GETTING_STARTED,
  parseGettingStarted,
  readGettingStarted,
  remainingSteps,
  satisfiedFactSteps,
  saveGettingStarted,
  showGettingStarted,
} from "./gettingStartedSteps";
import { readExportablePreferences } from "./userPreferences";

const allIds = GETTING_STARTED_STEPS.map((step) => step.id);

describe("getting started checklist", () => {
  afterEach(() => localStorage.removeItem(GETTING_STARTED_STORAGE_KEY));

  it("starts with every step open and the checklist shown", () => {
    expect(readGettingStarted()).toEqual(INITIAL_GETTING_STARTED);
    expect(remainingSteps(INITIAL_GETTING_STARTED)).toHaveLength(GETTING_STARTED_STEPS.length);
    expect(showGettingStarted(INITIAL_GETTING_STARTED)).toBe(true);
  });

  it("completes steps once and returns the same state when nothing changes", () => {
    const once = completeSteps(INITIAL_GETTING_STARTED, ["archive", "archive"]);
    expect(once.completed).toEqual(["archive"]);
    expect(completeSteps(once, ["archive"])).toBe(once);
  });

  it("derives fact-backed steps only from facts that hold", () => {
    expect(satisfiedFactSteps({ mailConnected: false, calendarConnected: false, aiReady: false })).toEqual([]);
    expect(satisfiedFactSteps({ mailConnected: true, calendarConnected: false, aiReady: true })).toEqual(["connectMail", "setUpAi"]);
  });

  it("hides once every step is done, including optional ones, or when dismissed", () => {
    const allButOptional = completeSteps(INITIAL_GETTING_STARTED, GETTING_STARTED_STEPS.filter((step) => !step.optional).map((step) => step.id));
    expect(showGettingStarted(allButOptional)).toBe(true);
    expect(showGettingStarted(completeSteps(INITIAL_GETTING_STARTED, allIds))).toBe(false);
    expect(showGettingStarted({ ...INITIAL_GETTING_STARTED, dismissed: true })).toBe(false);
  });

  it("round-trips through storage and ignores unknown or malformed values", () => {
    saveGettingStarted({ completed: ["connectMail", "shortcuts"], dismissed: true });
    expect(readGettingStarted()).toEqual({ completed: ["connectMail", "shortcuts"], dismissed: true });
    expect(parseGettingStarted('{"completed":["connectMail","retired",3,"connectMail"],"dismissed":"yes"}'))
      .toEqual({ completed: ["connectMail"], dismissed: false });
    expect(parseGettingStarted("not json")).toEqual(INITIAL_GETTING_STARTED);
    expect(parseGettingStarted("null")).toEqual(INITIAL_GETTING_STARTED);
  });

  it("stays on this device: progress is never part of settings export", () => {
    saveGettingStarted({ completed: ["setUpAi"], dismissed: true });
    const exported = JSON.stringify(readExportablePreferences());
    expect(exported).not.toMatch(/gettingStarted|setUpAi|dismissed/);
  });

  it("links every step to a docs page that the site publishes", () => {
    const published = new Set(readdirSync(path.resolve("site/docs")).filter((file) => file.endsWith(".md")).map((file) => file.slice(0, -3)));
    expect(published.has("index")).toBe(true);
    for (const step of GETTING_STARTED_STEPS) expect(published, step.id).toContain(step.docSlug);
    expect(docsUrl("ai")).toBe("https://www.threestrands.app/docs/ai.html");
    expect(docsUrl("triage", "undo")).toBe("https://www.threestrands.app/docs/triage.html#undo");
  });
});
