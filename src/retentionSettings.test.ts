import { beforeEach, describe, expect, it, vi } from "vitest";

describe("browser mail retention defaults", () => {
  beforeEach(() => {
    vi.resetModules();
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("defaults to 90 days and preserves a choice of Forever", async () => {
    const { getRetentionDays, setRetentionDays } = await import("./retentionSettings");
    expect(await getRetentionDays()).toBe(90);
    await setRetentionDays(null);
    expect(await getRetentionDays()).toBeNull();
    await setRetentionDays(365);
    expect(await getRetentionDays()).toBe(365);
  });
});
