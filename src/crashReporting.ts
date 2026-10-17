import { error as nativeLogError, warn as nativeLogWarn } from "@tauri-apps/plugin-log";
import type { CrashReport } from "./domain";

const CONSENT_KEY = "threestrands.crash-reporting.enabled";
const REPORTS_KEY = "threestrands.crash-reports";
const MAX_LOCAL_REPORTS = 20;

type ReporterOptions = {
  endpoint?: string;
  appVersion?: string;
};

type Reporter = (kind: CrashReport["kind"], reason: unknown) => void;

/** The installed reporter, so errors that never reach `window` can be reported too. */
let activeReporter: Reporter | null = null;

export function crashReportingEnabled(): boolean {
  return localStorage.getItem(CONSENT_KEY) === "true";
}

export function setCrashReportingEnabled(enabled: boolean): void {
  localStorage.setItem(CONSENT_KEY, String(enabled));
}

export function localCrashReports(): CrashReport[] {
  try {
    return JSON.parse(localStorage.getItem(REPORTS_KEY) ?? "[]") as CrashReport[];
  } catch {
    return [];
  }
}

export function clearLocalCrashReports(): void {
  localStorage.removeItem(REPORTS_KEY);
}

export function redactDiagnostic(value: string): string {
  return value
    .replace(/\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/gi, "[email]")
    .replace(/https?:\/\/[^\s)]+/gi, "[url]")
    // Defense in depth against a future error message that happens to
    // interpolate raw mail content: strip quoted strings and header-style
    // fragments (e.g. a MIME-build validation error quoting a subject or
    // recipient list) rather than relying solely on the two patterns above.
    .replace(/"[^"]*"/g, '"[redacted]"')
    .replace(/'[^']*'/g, "'[redacted]'")
    .replace(/\b(subject|to|cc|bcc|from)\s*:\s*[^\n]*/gi, "$1: [redacted]")
    .slice(0, 4_000);
}

/**
 * Text for a log line. WebKit's `stack` omits the message line that V8
 * includes, so the message always leads.
 */
function describeFailure(reason: unknown): string {
  if (!(reason instanceof Error)) return String(reason);
  return reason.stack && !reason.stack.startsWith(`${reason.name}: ${reason.message}`)
    ? `${reason.name}: ${reason.message}\n${reason.stack}`
    : reason.stack ?? `${reason.name}: ${reason.message}`;
}

/**
 * Writes a redacted line to the app's log file, next to the backend's own
 * entries. Release builds have no reachable webview console, so this is
 * where frontend failures stay diagnosable. Outside the desktop app (the
 * browser demo, unit tests) there is no log file and nothing is written.
 * Unlike crash reports this needs no consent: the log never leaves the
 * device unless the person shares it.
 */
export function writeNativeLog(level: "warn" | "error", message: string): void {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const write = level === "error" ? nativeLogError : nativeLogWarn;
  write(redactDiagnostic(message)).catch(() => {
    // A failed log write has nowhere left to be reported, and reporting it as
    // an unhandled rejection would loop back into the crash reporter.
  });
}

/**
 * Records an error that an error boundary caught. React does not rethrow
 * caught render errors to `window`, so without this they would never reach
 * the log file or the crash reporter.
 */
export function reportCaughtError(context: string, reason: unknown, componentStack?: string | null): void {
  const component = componentStack ? `\nComponent stack:${componentStack}` : "";
  writeNativeLog("error", `${context}: ${describeFailure(reason)}${component}`);
  activeReporter?.("error", reason);
}

function persist(report: CrashReport): void {
  const reports = [...localCrashReports(), report].slice(-MAX_LOCAL_REPORTS);
  localStorage.setItem(REPORTS_KEY, JSON.stringify(reports));
}

export function installCrashReporter({
  endpoint = import.meta.env.VITE_CRASH_REPORT_ENDPOINT,
  appVersion = import.meta.env.VITE_APP_VERSION ?? "development",
}: ReporterOptions = {}): () => void {
  const report: Reporter = (kind, reason) => {
    if (!crashReportingEnabled()) return;
    const error = reason instanceof Error ? reason : new Error(String(reason));
    const payload: CrashReport = {
      id: crypto.randomUUID(),
      occurredAt: new Date().toISOString(),
      kind,
      message: redactDiagnostic(error.message),
      stack: error.stack ? redactDiagnostic(error.stack) : null,
      appVersion,
      userAgent: navigator.userAgent,
    };
    persist(payload);
    if (endpoint) {
      void fetch(endpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
        keepalive: true,
      }).catch(() => {
        // Local retention is the reliable fallback; reporting must never crash
        // the application or block mail workflows.
      });
    }
  };
  const onError = (event: ErrorEvent) => {
    const reason = event.error ?? event.message;
    writeNativeLog("error", `Uncaught error: ${describeFailure(reason)}`);
    report("error", reason);
  };
  const onUnhandledRejection = (event: PromiseRejectionEvent) => {
    writeNativeLog("error", `Unhandled rejection: ${describeFailure(event.reason)}`);
    report("unhandledrejection", event.reason);
  };
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onUnhandledRejection);
  activeReporter = report;
  return () => {
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onUnhandledRejection);
    if (activeReporter === report) activeReporter = null;
  };
}
