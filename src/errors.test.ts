import { afterEach, describe, expect, it, vi } from "vitest";
import { warn as nativeLogWarn } from "@tauri-apps/plugin-log";
import { errorMessage, logBackgroundFailure } from "./errors";

vi.mock("@tauri-apps/plugin-log", () => ({
  error: vi.fn(() => Promise.resolve()),
  warn: vi.fn(() => Promise.resolve()),
}));

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

  it("also writes the failure to the app's log file in the desktop app", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.mocked(nativeLogWarn).mockClear();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    try {
      await Promise.reject(new Error("offline")).catch(logBackgroundFailure("Unread count refresh"));
      await Promise.reject("Gmail is unreachable").catch(logBackgroundFailure("Sync flush"));
    } finally {
      Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    }

    expect(nativeLogWarn).toHaveBeenCalledWith("Unread count refresh failed: offline");
    expect(nativeLogWarn).toHaveBeenCalledWith("Sync flush failed: Gmail is unreachable");
  });

  it("settles the chain so the rejection is not reported as unhandled", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});

    await expect(Promise.reject("offline").catch(logBackgroundFailure("Sync flush"))).resolves.toBeUndefined();
  });
});
