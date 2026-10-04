import { describe, expect, it } from "vitest";
import { applySelectionGesture, selectionGestureFor } from "./threadSelection";

const orderedIds = ["a", "b", "c", "d", "e"];
const none = new Set<string>();

describe("threadSelection", () => {
  it("maps modifier keys to gestures", () => {
    expect(selectionGestureFor({ metaKey: false, ctrlKey: false, shiftKey: false })).toBeNull();
    expect(selectionGestureFor({ metaKey: true, ctrlKey: false, shiftKey: false })).toBe("toggle");
    expect(selectionGestureFor({ metaKey: false, ctrlKey: true, shiftKey: false })).toBe("toggle");
    expect(selectionGestureFor({ metaKey: false, ctrlKey: false, shiftKey: true })).toBe("range");
    expect(selectionGestureFor({ metaKey: true, ctrlKey: false, shiftKey: true })).toBe("range-add");
  });

  it("seeds a toggle with the open conversation and toggles the target", () => {
    const base = { gesture: "toggle" as const, anchorId: "b", openId: "b", orderedIds };
    expect([...applySelectionGesture({ ...base, targetId: "d", checked: none })]).toEqual(["b", "d"]);
    expect([...applySelectionGesture({ ...base, targetId: "d", checked: new Set(["b", "d"]) })]).toEqual(["b"]);
    expect([...applySelectionGesture({ ...base, targetId: "b", checked: none })]).toEqual(["b"]);
    expect([...applySelectionGesture({ ...base, openId: "zz", targetId: "d", checked: none })]).toEqual(["d"]);
  });

  it("selects the inclusive range from the anchor in either direction", () => {
    const base = { gesture: "range" as const, openId: null, orderedIds, checked: new Set(["e"]) };
    expect([...applySelectionGesture({ ...base, anchorId: "b", targetId: "d" })]).toEqual(["b", "c", "d"]);
    expect([...applySelectionGesture({ ...base, anchorId: "d", targetId: "a" })]).toEqual(["a", "b", "c", "d"]);
    expect([...applySelectionGesture({ ...base, anchorId: "c", targetId: "c" })]).toEqual(["c"]);
  });

  it("adds the range to the existing set with range-add", () => {
    const result = applySelectionGesture({
      gesture: "range-add", anchorId: "c", targetId: "d", openId: null, orderedIds, checked: new Set(["a"]),
    });
    expect([...result].sort()).toEqual(["a", "c", "d"]);
  });

  it("falls back to the target alone when the anchor is not visible", () => {
    const result = applySelectionGesture({
      gesture: "range", anchorId: "gone", targetId: "c", openId: null, orderedIds, checked: none,
    });
    expect([...result]).toEqual(["c"]);
  });
});
