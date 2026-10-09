import { useCallback } from "react";
import type { SummaryResult } from "./domain";
import type { useMailboxThreads } from "./useMailboxThreads";
import type { useThreadDetail } from "./useThreadDetail";

type Options = {
  mailboxThreads: Pick<ReturnType<typeof useMailboxThreads>, "setThreads">;
  threadDetail: Pick<ReturnType<typeof useThreadDetail>, "setDetail">;
};

/** Coordinates changes shared by mailbox rows and the open conversation. */
export function useMailStateActions({ mailboxThreads: { setThreads }, threadDetail: { setDetail } }: Options) {
  const applyThreadSummary = useCallback((threadId: string, result: SummaryResult) => {
    const summary = {
      summary: result.summary,
      summaryGeneratedAt: result.generatedAt,
      summaryRevision: result.revision,
    };
    // A request can finish after navigation or a refresh. Update only matching
    // entries in the current state; never restore an absent row or replace a
    // different conversation's detail.
    setThreads((current) =>
      current.map((thread) => thread.id === threadId ? { ...thread, ...summary } : thread),
    );
    setDetail((current) =>
      current?.thread.id === threadId
        ? { ...current, thread: { ...current.thread, ...summary } }
        : current,
    );
  }, [setDetail, setThreads]);

  return { applyThreadSummary };
}
