import { describe, expect, it } from "vitest";
import {
  buildTriageCloseEvent,
  buildTriageDispositionEvent,
  isQuickTriageDisposition,
  pauseTriageSession,
  resumeTriageSession,
} from "./triage";

describe("triage signal classification", () => {
  const session = {
    threadId: "thread-1",
    context: "inbox" as const,
    startedAt: 1000,
    activeElapsedMs: 0,
    active: true,
    scrolled: false,
  };

  it("classifies a short, unscrolled single-message dismissal as quick", () => {
    const event = buildTriageDispositionEvent({
      threadId: "thread-1",
      action: "archive",
      context: "inbox",
      session,
      now: 2000,
      batch: false,
    });
    expect(event.dwellMs).toBe(1000);
    expect(isQuickTriageDisposition(event)).toBe(true);
  });

  it("does not classify scrolling, batching, or longer handling as quick", () => {
    expect(isQuickTriageDisposition(buildTriageDispositionEvent({
      threadId: "thread-1",
      action: "trash",
      context: "inbox",
      session: { ...session, scrolled: true },
      now: 1200,
      batch: false,
    }))).toBe(false);
    expect(isQuickTriageDisposition(buildTriageDispositionEvent({
      threadId: "thread-1",
      action: "trash",
      context: "inbox",
      session,
      now: 1200,
      batch: true,
    }))).toBe(false);
    expect(isQuickTriageDisposition(buildTriageDispositionEvent({
      threadId: "thread-1",
      action: "trash",
      context: "inbox",
      session,
      now: 2001,
      batch: false,
    }))).toBe(false);
  });

  it("records list or batch dispositions without pretending the message was opened", () => {
    const event = buildTriageDispositionEvent({
      threadId: "thread-1",
      action: "archive",
      context: "inbox",
      session: null,
      now: 2000,
      batch: true,
    });
    expect(event.opened).toBe(false);
    expect(event.dwellMs).toBeNull();
    expect(isQuickTriageDisposition(event)).toBe(false);
  });

  it("captures engaged close state independently of disposition", () => {
    expect(buildTriageCloseEvent({ ...session, scrolled: true }, 1300)).toEqual({
      threadId: "thread-1",
      kind: "close",
      context: "inbox",
      dwellMs: 300,
      scrolled: true,
    });
  });

  it("measures active time across background pauses", () => {
    const paused = { ...session };
    pauseTriageSession(paused, 1400);
    resumeTriageSession(paused, 5000);
    expect(buildTriageCloseEvent(paused, 5300).dwellMs).toBe(700);
  });
});
