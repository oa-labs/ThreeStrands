import { describe, expect, it } from "vitest";
import type { Note } from "./domain";
import type { FrontierConflict } from "./replicatedSync";
import { MAX_DIFF_CELLS, conflictChoices, deviceName, diffLines, noteConflictView, noteConflictsById } from "./noteConflicts";

const note: Note = { id: "n1", title: "Plan", body: "shown body", dailyDate: null, createdAt: "2026-11-01T00:00:00Z", updatedAt: "2026-11-02T00:00:00Z", trashed: false, trashedAt: null };
const conflict = (field: string, values: [string, unknown][], entityId = "n1", entityType = "note"): FrontierConflict => ({
  entityType, entityId, field,
  candidates: values.map(([deviceId, value], index) => ({ operationId: `${deviceId}:${index + 1}`, deviceId, value })),
});

describe("note conflicts", () => {
  it("groups the fields a note version shows by note", () => {
    const grouped = noteConflictsById([
      conflict("body", [["a", "x"], ["b", "y"]]),
      conflict("title", [["a", "Plan"], ["b", "Plans"]]),
      conflict("body", [["a", "x"], ["b", "y"]], "n2"),
      conflict("createdAt", [["a", "1"], ["b", "2"]]),
      conflict("title", [["a", "x"], ["b", "y"]], "t1", "task"),
    ]);
    expect([...grouped.keys()]).toEqual(["n1", "n2"]);
    expect(grouped.get("n1")!.map((item) => item.field)).toEqual(["body", "title"]);
  });

  it("builds each other device's version from its values and the shown ones", () => {
    const conflicts = [
      conflict("body", [["a", "shown body"], ["b", "other body"]]),
      conflict("title", [["a", "Plan"], ["c", "Plan B"]]),
    ];
    const view = noteConflictView(note, conflicts);
    expect(view.shownDeviceId).toBe("a");
    expect(view.others).toEqual([
      { deviceId: "b", title: "Plan", body: "other body", trashed: false },
      { deviceId: "c", title: "Plan B", body: "shown body", trashed: false },
    ]);
  });

  it("reads a trash conflict and a removed value", () => {
    const view = noteConflictView(note, [conflict("trashed", [["a", false], ["b", true]]), conflict("body", [["a", "shown body"], ["b", null]])]);
    expect(view.others).toEqual([{ deviceId: "b", title: "Plan", body: "", trashed: true }]);
  });

  it("chooses a device's value per field and the shown value where it wrote none", () => {
    const conflicts = [
      conflict("body", [["a", "shown body"], ["b", "other body"]]),
      conflict("title", [["a", "Plan"], ["c", "Plan B"]]),
    ];
    expect(conflictChoices(note, conflicts, "b").map(({ chosen }) => chosen.operationId)).toEqual(["b:2", "a:1"]);
    expect(conflictChoices(note, conflicts, null).map(({ chosen }) => chosen.operationId)).toEqual(["a:1", "a:1"]);
  });

  it("names devices as Settings does", () => {
    const roster = [
      { deviceId: "a", status: "active", isSelf: true, label: null },
      { deviceId: "b", status: "active", isSelf: false, label: "Work laptop" },
    ];
    expect(deviceName("a", roster)).toBe("this device");
    expect(deviceName("b", roster)).toBe("Work laptop");
    expect(deviceName("z", roster)).toBe("another device");
  });
});

describe("diffLines", () => {
  it("marks lines only in one text", () => {
    expect(diffLines("a\nb\nc\nd", "a\nc\nx\nd")).toEqual([
      { kind: "same", text: "a" },
      { kind: "removed", text: "b" },
      { kind: "same", text: "c" },
      { kind: "added", text: "x" },
      { kind: "same", text: "d" },
    ]);
    expect(diffLines("same", "same")).toEqual([{ kind: "same", text: "same" }]);
  });

  it("aligns lines up to the size limit and marks the whole middle as changed above it", () => {
    const side = Math.sqrt(MAX_DIFF_CELLS);
    const shared = (count: number) => Array.from({ length: count }, (_, index) => `line ${index}`);
    // Both middles share every line but one, shifted by one place.
    const texts = (middle: number) => [
      ["top", "only before", ...shared(middle - 1), "end"].join("\n"),
      ["top", ...shared(middle - 1), "only after", "end"].join("\n"),
    ] as const;
    const count = (lines: ReturnType<typeof diffLines>, kind: string) => lines.filter((line) => line.kind === kind).length;

    const below = diffLines(...texts(side - 1));
    expect([count(below, "removed"), count(below, "added")]).toEqual([1, 1]);
    const at = diffLines(...texts(side));
    expect([count(at, "removed"), count(at, "added")]).toEqual([1, 1]);
    const above = diffLines(...texts(side + 1));
    expect([count(above, "removed"), count(above, "added")]).toEqual([side + 1, side + 1]);
    expect(above[0]).toEqual({ kind: "same", text: "top" });
    expect(above.at(-1)).toEqual({ kind: "same", text: "end" });
  });
});
