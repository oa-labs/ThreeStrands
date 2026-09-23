import type { ReactNode } from "react";
import type { ReplicatedSyncTransportStatus } from "./replicatedSync";
import type { useSettingsOperation } from "./settingsOperations";

/** Shared building blocks for the Replicated Sync settings screens. */

export type Operation = ReturnType<typeof useSettingsOperation>;

export function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

export function formatStorageEstimate(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(1)} ${units[index]}`;
}

/** A collapsible settings group. Content sits in its own flex body because
 * WebKit does not lay out `<details>` children as flex items. */
export function Disclosure({
  summary,
  open,
  className,
  children,
}: {
  summary: ReactNode;
  open?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <details className={className ? `settings-disclosure ${className}` : "settings-disclosure"} open={open}>
      <summary>{summary}</summary>
      <div className="settings-disclosure-body">{children}</div>
    </details>
  );
}

/** The failure from one keyed operation, shown next to its control. */
export function InlineStatus({ operation, for: key }: { operation: Operation; for: string }) {
  return operation.error && operation.errorKey === key
    ? <p role="status" className="settings-hint settings-inline-status">{operation.error}</p>
    : null;
}

export type Tone = "ok" | "attention";
export type TransportHealthSummary = { tone: Tone; label: string; detail: string | null };

/** Plain-language state for one connector. The native side reports
 * `healthy`, `degraded: <reason>`, or `unavailable: <reason>`. */
export function describeTransportHealth(transport: ReplicatedSyncTransportStatus): TransportHealthSummary {
  const separator = transport.health.indexOf(": ");
  const state = separator === -1 ? transport.health : transport.health.slice(0, separator);
  const reason = separator === -1 ? null : transport.health.slice(separator + 2);
  if (state === "unavailable") {
    return reason === "not configured"
      ? { tone: "attention", label: "Not set up correctly. Disconnect it and add it again.", detail: null }
      : { tone: "attention", label: "Can’t reach this connector", detail: reason };
  }
  if (state === "degraded") return { tone: "attention", label: "Having trouble, retrying automatically", detail: reason };
  if (state !== "healthy") return { tone: "attention", label: "Status unknown", detail: transport.health };
  if (transport.failed > 0) {
    return { tone: "attention", label: `${plural(transport.failed, "change")} couldn’t be uploaded`, detail: transport.lastError ?? null };
  }
  if (transport.pending > 0) return { tone: "ok", label: `Uploading ${plural(transport.pending, "change")}`, detail: null };
  return { tone: "ok", label: "Up to date", detail: null };
}

/** "in 5 hours", "in 3 minutes", "in 2 days", or "now" once past. */
export function formatTimeUntil(iso: string, now: number = Date.now()): string {
  const remaining = new Date(iso).getTime() - now;
  if (!Number.isFinite(remaining) || remaining <= 0) return "now";
  const minutes = Math.ceil(remaining / 60_000);
  if (minutes < 60) return `in ${plural(minutes, "minute")}`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `in ${plural(hours, "hour")}`;
  return `in ${plural(Math.round(hours / 24), "day")}`;
}
