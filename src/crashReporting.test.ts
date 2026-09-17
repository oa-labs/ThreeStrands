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

  it("redacts quoted strings and header-style fragments as defense in depth", () => {
    expect(redactDiagnostic('invalid header value "Q4 roadmap review"')).toBe(
      'invalid header value "[redacted]"',
    );
    expect(redactDiagnostic("Subject: Q4 roadmap review\nTo: someone@example.com")).toBe(
      "Subject: [redacted]\nTo: [redacted]",
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
