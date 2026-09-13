import type { TriageEvent } from "./domain";

export const TRIAGE_QUICK_DISMISSAL_MAX_DWELL_MS = 1000;

export type TriageSession = {
  threadId: string;
  context: TriageEvent["context"];
  startedAt: number;
  activeElapsedMs: number;
  active: boolean;
  scrolled: boolean;
};

export function pauseTriageSession(session: TriageSession, now: number): void {
  if (!session.active) return;
  session.activeElapsedMs += Math.max(0, now - session.startedAt);
  session.active = false;
}

export function resumeTriageSession(session: TriageSession, now: number): void {
  if (session.active) return;
  session.startedAt = now;
  session.active = true;
}

function triageDwellMs(session: TriageSession, now: number): number {
  const activeMs = session.active
    ? session.activeElapsedMs + Math.max(0, now - session.startedAt)
    : session.activeElapsedMs;
  return Math.round(activeMs);
}

export function buildTriageDispositionEvent({
  threadId,
  action,
  context,
  session,
  now,
  batch,
}: {
  threadId: string;
  action: "archive" | "trash";
  context: TriageEvent["context"];
  session: TriageSession | null;
  now: number;
  batch: boolean;
}): TriageEvent {
  const active = session?.threadId === threadId ? session : null;
  return {
    threadId,
    kind: "disposition",
    context: active?.context ?? context,
    action,
    opened: Boolean(active),
    dwellMs: active ? triageDwellMs(active, now) : null,
    scrolled: active?.scrolled ?? false,
    batch,
  };
}

export function isQuickTriageDisposition(event: TriageEvent): boolean {
  return event.kind === "disposition"
    && event.action !== undefined
    && event.opened === true
    && event.batch !== true
    && event.scrolled !== true
    && event.dwellMs !== null
    && event.dwellMs !== undefined
    && event.dwellMs >= 0
    && event.dwellMs <= TRIAGE_QUICK_DISMISSAL_MAX_DWELL_MS;
}

export function buildTriageCloseEvent(session: TriageSession, now: number): TriageEvent {
  return {
    threadId: session.threadId,
    kind: "close",
    context: session.context,
    dwellMs: triageDwellMs(session, now),
    scrolled: session.scrolled,
  };
}
