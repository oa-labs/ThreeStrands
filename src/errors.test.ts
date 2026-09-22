import { afterEach, describe, expect, it, vi } from "vitest";
import { errorMessage, logBackgroundFailure } from "./errors";

describe("errorMessage", () => {
  it("uses an Error's message without the constructor prefix", () => {
    expect(errorMessage(new Error("offline"))).toBe("offline");
    expect(errorMessage(new TypeError("bad input"))).toBe("bad input");
  });

  it("passes Tauri string rejections through unchanged", () => {
    expect(errorMessage("Google sign-in was cancelled")).toBe("Google sign-in was cancelled");
  });

  it("stringifies other rejection values", () => {
    expect(errorMessage(42)).toBe("42");
    expect(errorMessage(null)).toBe("null");
    expect(errorMessage(undefined)).toBe("undefined");
  });
});

describe("logBackgroundFailure", () => {
  afterEach(() => vi.restoreAllMocks());

  it("logs the failed task and reason instead of discarding them", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const reason = new Error("offline");

    await Promise.reject(reason).catch(logBackgroundFailure("Unread count refresh"));

    expect(warn).toHaveBeenCalledWith("Unread count refresh failed:", reason);
  });

  it("settles the chain so the rejection is not reported as unhandled", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});

    await expect(Promise.reject("offline").catch(logBackgroundFailure("Sync flush"))).resolves.toBeUndefined();
  });
});
