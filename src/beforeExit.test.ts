import { describe, expect, it, vi } from "vitest";
import { onBeforeExit, runBeforeExitTasks } from "./beforeExit";

describe("before-exit tasks", () => {
  it("runs every registered task, even after one fails, and forgets removed ones", async () => {
    const saved = vi.fn(async () => {});
    const removed = vi.fn(async () => {});
    const stopFailing = onBeforeExit(async () => { throw new Error("offline"); });
    const stopSaved = onBeforeExit(saved);
    onBeforeExit(removed)();
    await expect(runBeforeExitTasks()).resolves.toBeUndefined();
    expect(saved).toHaveBeenCalledTimes(1);
    expect(removed).not.toHaveBeenCalled();
    stopFailing(); stopSaved();
  });
});
