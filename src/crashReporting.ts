import type { CrashReport } from "./domain";

const CONSENT_KEY = "dispatch.crash-reporting.enabled";
const REPORTS_KEY = "dispatch.crash-reports";
const MAX_LOCAL_REPORTS = 20;

type ReporterOptions = {
  endpoint?: string;
  appVersion?: string;
};

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
    .slice(0, 4_000);
}

function persist(report: CrashReport): void {
  const reports = [...localCrashReports(), report].slice(-MAX_LOCAL_REPORTS);
  localStorage.setItem(REPORTS_KEY, JSON.stringify(reports));
}

export function installCrashReporter({
  endpoint = import.meta.env.VITE_CRASH_REPORT_ENDPOINT,
  appVersion = import.meta.env.VITE_APP_VERSION ?? "development",
}: ReporterOptions = {}): () => void {
  const report = (kind: CrashReport["kind"], reason: unknown) => {
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
  const onError = (event: ErrorEvent) => report("error", event.error ?? event.message);
  const onUnhandledRejection = (event: PromiseRejectionEvent) =>
    report("unhandledrejection", event.reason);
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onUnhandledRejection);
  return () => {
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onUnhandledRejection);
  };
}
