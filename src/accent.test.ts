import { afterEach, describe, expect, it, vi } from "vitest";
import { applyAccent, DEFAULT_ACCENT, readAccent, saveAccent } from "./accent";

afterEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.accent;
});

describe("accent", () => {
  it("falls back to the brand accent when nothing valid is stored", () => {
    expect(readAccent()).toBe(DEFAULT_ACCENT);
    localStorage.setItem("threestrands.accent", "neon");
    expect(readAccent()).toBe(DEFAULT_ACCENT);
  });

  it("round-trips a saved accent through storage and the document", () => {
    saveAccent("teal");
    expect(readAccent()).toBe("teal");
    expect(document.documentElement.dataset.accent).toBe("teal");
  });

  it("keeps the picker working when storage is unavailable", () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(() => saveAccent("rose")).not.toThrow();
    expect(document.documentElement.dataset.accent).toBe("rose");
    setItem.mockRestore();
  });

  it("applies an accent without touching storage", () => {
    applyAccent("graphite");
    expect(document.documentElement.dataset.accent).toBe("graphite");
    expect(localStorage.getItem("threestrands.accent")).toBeNull();
  });
});
