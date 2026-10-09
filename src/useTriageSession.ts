import { useCallback, useEffect, useRef, type RefObject } from "react";
import type { MailboxKind } from "./commands";
import type { ThreadDetail, TriageEvent } from "./domain";
import { mailClient } from "./data/client";
import { logBackgroundFailure } from "./errors";
import { triageNow } from "./threadPresentation";
import { buildTriageCloseEvent, pauseTriageSession, resumeTriageSession, type TriageSession } from "./triage";

type Options = {
  visibleDetail: ThreadDetail | null;
  mailbox: MailboxKind;
  includeArchived: boolean;
  messageStackRef: RefObject<HTMLDivElement | null>;
};

export function useTriageSession({ visibleDetail, mailbox, includeArchived, messageStackRef }: Options) {
  const triageSessionRef = useRef<TriageSession | null>(null);
  const triageCloseTimerRef = useRef<number | null>(null);
  const recordTriageEvent = useCallback((event: TriageEvent) => {
    // Instrumentation is deliberately best-effort: a local telemetry write
    // must never make a mail action or navigation fail.
    void mailClient.recordTriageEvent(event).catch(logBackgroundFailure("Triage event recording"));
  }, []);
  useEffect(() => {
    if (!visibleDetail) return;
    const threadId = visibleDetail.thread.id;
    const context: TriageEvent["context"] = mailbox === "inbox" && !includeArchived ? "inbox" : "other";
    if (triageCloseTimerRef.current !== null) {
      window.clearTimeout(triageCloseTimerRef.current);
      triageCloseTimerRef.current = null;
    }
    const existing = triageSessionRef.current;
    if (existing && (existing.threadId !== threadId || existing.context !== context)) {
      recordTriageEvent(buildTriageCloseEvent(existing, triageNow()));
      triageSessionRef.current = null;
    }
    const session = triageSessionRef.current ?? {
      threadId,
      context,
      startedAt: triageNow(),
      activeElapsedMs: 0,
      active: true,
      scrolled: false,
    };
    if (triageSessionRef.current !== session) {
      triageSessionRef.current = session;
      recordTriageEvent({
        threadId: session.threadId,
        kind: "open",
        context: session.context,
      });
    }

    const messageStack = messageStackRef.current;
    const onScroll = () => {
      if (messageStack && messageStack.scrollTop > 8) session.scrolled = true;
    };
    const onPause = () => pauseTriageSession(session, triageNow());
    const onResume = () => resumeTriageSession(session, triageNow());
    const onVisibilityChange = () => {
      if (document.visibilityState === "hidden") onPause();
      else onResume();
    };
    messageStack?.addEventListener("scroll", onScroll, { passive: true });
    document.addEventListener("visibilitychange", onVisibilityChange);
    window.addEventListener("blur", onPause);
    window.addEventListener("focus", onResume);
    return () => {
      messageStack?.removeEventListener("scroll", onScroll);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      window.removeEventListener("blur", onPause);
      window.removeEventListener("focus", onResume);
      if (triageSessionRef.current !== session) return;
      // React Strict Mode replays effects immediately in development. Delay
      // the close one tick so the next setup can reuse the same session.
      triageCloseTimerRef.current = window.setTimeout(() => {
        triageCloseTimerRef.current = null;
        if (triageSessionRef.current !== session) return;
        recordTriageEvent(buildTriageCloseEvent(session, triageNow()));
        triageSessionRef.current = null;
      }, 0);
    };
  }, [includeArchived, mailbox, messageStackRef, recordTriageEvent, visibleDetail]);

  return { triageSessionRef, recordTriageEvent };
}
