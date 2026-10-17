import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { error as nativeLogError, warn as nativeLogWarn } from "@tauri-apps/plugin-log";
import {
  crashReportingEnabled,
  installCrashReporter,
  localCrashReports,
  redactDiagnostic,
  reportCaughtError,
  setCrashReportingEnabled,
  writeNativeLog,
} from "./crashReporting";

vi.mock("@tauri-apps/plugin-log", () => ({
  error: vi.fn(() => Promise.resolve()),
  warn: vi.fn(() => Promise.resolve()),
}));

function runAsDesktopApp() {
  Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
}

function runInBrowser() {
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
}

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

describe("native log", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(nativeLogError).mockClear();
    vi.mocked(nativeLogWarn).mockClear();
    runAsDesktopApp();
  });
  afterEach(runInBrowser);

  it("writes redacted lines to the app's log file at the requested level", () => {
    writeNativeLog("warn", "Unread count refresh failed: me@private.example");
    writeNativeLog("error", 'Rendering failed near "Q4 roadmap review"');

    expect(nativeLogWarn).toHaveBeenCalledWith("Unread count refresh failed: [email]");
    expect(nativeLogError).toHaveBeenCalledWith('Rendering failed near "[redacted]"');
  });

  it("writes nothing outside the desktop app", () => {
    runInBrowser();
    writeNativeLog("warn", "Sync flush failed: offline");
    expect(nativeLogWarn).not.toHaveBeenCalled();
  });

  it("swallows a failed log write instead of raising an unhandled rejection", async () => {
    vi.mocked(nativeLogWarn).mockRejectedValueOnce(new Error("log plugin unavailable"));
    const unhandled = vi.fn();
    window.addEventListener("unhandledrejection", unhandled);

    expect(() => writeNativeLog("warn", "Sync flush failed")).not.toThrow();
    await new Promise((resolve) => setTimeout(resolve, 0));

    window.removeEventListener("unhandledrejection", unhandled);
    expect(unhandled).not.toHaveBeenCalled();
  });

  it("logs uncaught errors and unhandled rejections even without crash-report consent", () => {
    const remove = installCrashReporter({ appVersion: "test" });
    window.dispatchEvent(new ErrorEvent("error", { error: new TypeError("thread is undefined") }));
    const rejection = new Event("unhandledrejection") as PromiseRejectionEvent;
    Object.defineProperty(rejection, "reason", { value: "offline" });
    window.dispatchEvent(rejection);
    remove();

    expect(nativeLogError).toHaveBeenCalledWith(expect.stringContaining("Uncaught error: TypeError: thread is undefined"));
    expect(nativeLogError).toHaveBeenCalledWith("Unhandled rejection: offline");
    expect(localCrashReports()).toHaveLength(0);
  });
});

describe("reportCaughtError", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(nativeLogError).mockClear();
    runAsDesktopApp();
  });
  afterEach(runInBrowser);

  it("logs the context, message, and component stack", () => {
    reportCaughtError("Showing this conversation failed", new TypeError("cannot read 'body'"), "\n    in MessageCard");

    const [line] = vi.mocked(nativeLogError).mock.calls[0]!;
    expect(line).toMatch(/^Showing this conversation failed: TypeError: cannot read '\[redacted\]'/);
    expect(line).toContain("Component stack:\n    in MessageCard");
  });

  it("leads with the message when the stack omits it, as WebKit's does", () => {
    const error = new Error("bad header");
    error.stack = "render@tauri://localhost/assets/index.js:1:1";
    reportCaughtError("Showing Settings failed", error);

    expect(nativeLogError).toHaveBeenCalledWith("Showing Settings failed: Error: bad header\nrender@tauri://localhost/assets/index.js:1:1");
  });

  it("files a crash report through the installed reporter only after opt-in", () => {
    const remove = installCrashReporter({ appVersion: "test" });
    reportCaughtError("Showing the inbox failed", new Error("from me@private.example"));
    expect(localCrashReports()).toHaveLength(0);

    setCrashReportingEnabled(true);
    reportCaughtError("Showing the inbox failed", new Error("from me@private.example"));
    remove();

    expect(localCrashReports()).toHaveLength(1);
    expect(localCrashReports()[0]?.message).toBe("from [email]");
  });

  it("stops filing crash reports once the reporter is removed", () => {
    setCrashReportingEnabled(true);
    installCrashReporter({ appVersion: "test" })();
    reportCaughtError("Showing the inbox failed", new Error("boom"));

    expect(localCrashReports()).toHaveLength(0);
    expect(nativeLogError).toHaveBeenCalled();
  });
});
