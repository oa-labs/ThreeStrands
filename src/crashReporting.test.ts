import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  crashReportingEnabled,
  installCrashReporter,
  localCrashReports,
  redactDiagnostic,
  setCrashReportingEnabled,
} from "./crashReporting";

describe("crash reporting", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.restoreAllMocks());

  it("is disabled by default", () => {
    expect(crashReportingEnabled()).toBe(false);
  });

  it("redacts addresses and URLs", () => {
    expect(redactDiagnostic("mail me@private.example from https://private.example/a")).toBe(
      "mail [email] from [url]",
    );
  });

  it("records sanitized errors only after opt-in", () => {
    const remove = installCrashReporter({ appVersion: "test" });
    window.dispatchEvent(new ErrorEvent("error", { error: new Error("from me@private.example") }));
    expect(localCrashReports()).toHaveLength(0);

    setCrashReportingEnabled(true);
    window.dispatchEvent(new ErrorEvent("error", { error: new Error("from me@private.example") }));
    expect(localCrashReports()).toHaveLength(1);
    expect(localCrashReports()[0]?.message).toBe("from [email]");
    remove();
  });
});
