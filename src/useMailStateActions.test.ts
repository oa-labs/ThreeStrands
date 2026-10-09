import { useState } from "react";
import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { SummaryResult, Thread, ThreadDetail } from "./domain";
import { useMailStateActions } from "./useMailStateActions";

function thread(id: string): Thread {
  return {
    id,
    providerThreadId: id,
    subject: `Subject ${id}`,
    snippet: `Snippet ${id}`,
    participants: ["sender@example.com"],
    lastMessageAt: "2026-10-09T12:00:00Z",
    lastReceivedAt: "2026-10-09T12:00:00Z",
    unread: true,
    starred: false,
    archived: false,
    trashed: false,
    labels: ["INBOX"],
    accountId: "account@example.com",
    summary: null,
    summaryGeneratedAt: null,
    summaryRevision: null,
    hasAttachments: false,
  };
}

const first = thread("first");
const second = thread("second");
const firstDetail: ThreadDetail = { thread: first, messages: [] };
const secondDetail: ThreadDetail = { thread: second, messages: [] };
const summary: SummaryResult = {
  summary: "A summary of the first conversation.",
  generatedAt: "2026-10-09T12:05:00Z",
  revision: first.lastMessageAt,
};
const summaryFields = {
  summary: summary.summary,
  summaryGeneratedAt: summary.generatedAt,
  summaryRevision: summary.revision,
};

function setup() {
  return renderHook(() => {
    const [threads, setThreads] = useState([first, second]);
    const [detail, setDetail] = useState<ThreadDetail | null>(firstDetail);
    const actions = useMailStateActions({ mailboxThreads: { setThreads }, threadDetail: { setDetail } });
    return { threads, detail, setThreads, setDetail, ...actions };
  });
}

describe("useMailStateActions", () => {
  it("applies the same summary metadata to the matching row and detail, preserving other fields and messages", () => {
    const { result } = setup();

    act(() => result.current.applyThreadSummary(first.id, summary));

    expect(result.current.threads).toEqual([{ ...first, ...summaryFields }, second]);
    expect(result.current.threads[1]).toBe(second);
    expect(result.current.detail).toEqual({ ...firstDetail, thread: { ...first, ...summaryFields } });
    expect(result.current.detail?.messages).toBe(firstDetail.messages);
  });

  it("uses current state when a captured action finishes after switching conversations", () => {
    const { result } = setup();
    const applyPendingSummary = result.current.applyThreadSummary;
    act(() => {
      result.current.setDetail(secondDetail);
      result.current.setThreads((current) => current.map((row) => ({ ...row, starred: true })));
    });

    act(() => applyPendingSummary(first.id, summary));

    expect(result.current.threads).toEqual([
      { ...first, starred: true, ...summaryFields },
      { ...second, starred: true },
    ]);
    expect(result.current.detail).toBe(secondDetail);
  });

  it.each([null, secondDetail])("does not restore an absent row or unrelated detail after leaving its view (detail: %j)", (detail) => {
    const { result } = setup();
    const applyPendingSummary = result.current.applyThreadSummary;
    act(() => {
      result.current.setThreads([second]);
      result.current.setDetail(detail);
    });

    act(() => applyPendingSummary(first.id, summary));

    expect(result.current.threads).toEqual([second]);
    expect(result.current.detail).toBe(detail);
  });

  it("updates an open conversation outside the current mailbox list without inserting a row", () => {
    const { result } = setup();
    act(() => result.current.setThreads([second]));

    act(() => result.current.applyThreadSummary(first.id, summary));

    expect(result.current.threads).toEqual([second]);
    expect(result.current.detail?.thread).toEqual({ ...first, ...summaryFields });
  });
});
