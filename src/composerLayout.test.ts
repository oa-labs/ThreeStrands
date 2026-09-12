import { afterEach, describe, expect, it } from "vitest";
import {
  clampComposerPosition,
  composerPositionKey,
  composerSizeKey,
  readComposerPosition,
  readComposerSize,
  saveComposerPosition,
  saveComposerSize,
} from "./composerLayout";

describe("composer layout", () => {
  afterEach(() => localStorage.clear());

  it("keeps a dragged window fully on screen", () => {
    expect(clampComposerPosition({ x: -80, y: -40 }, { width: 500, height: 400 }, { width: 1000, height: 800 }))
      .toEqual({ x: 0, y: 0 });
    expect(clampComposerPosition({ x: 900, y: 700 }, { width: 500, height: 400 }, { width: 1000, height: 800 }))
      .toEqual({ x: 500, y: 400 });
  });

  it("round-trips size and position through storage", () => {
    expect(readComposerSize()).toBeNull();
    expect(readComposerPosition()).toBeNull();
    saveComposerSize({ width: 640, height: 480 });
    saveComposerPosition({ x: 24, y: 36 });
    expect(readComposerSize()).toEqual({ width: 640, height: 480 });
    expect(readComposerPosition()).toEqual({ x: 24, y: 36 });
    expect(localStorage.getItem(composerSizeKey)).toContain("640");
    expect(localStorage.getItem(composerPositionKey)).toContain("36");
  });
});
