import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { InboxAgentsSnapshot } from "./domain";
import { useInboxAgents } from "./useInboxAgents";

function sampleSnapshot(): InboxAgentsSnapshot {
  return {
    runnerEnabled: true,
    agents: [
      {
        agentId: "stale_reads",
        name: "Stale Reads Summarizer",
        description: "Summarizes stale reads.",
        enabled: true,
        ageDays: 7,
        maxThreadsPerRun: 15,
        includeStarred: false,
        accountId: null,
        scheduleHours: null,
        lastRunAt: null,
        lastRunStatus: null,
        lastRunError: null,
        lastRunScanned: 0,
        lastRunProposed: 0,
        updatedAt: 0,
      },
    ],
    activeRuns: [],
    proposals: [
      {
        id: "prop-1",
        agentId: "stale_reads",
        accountId: "you@example.com",
        threadId: "thread-1",
        threadRevision: "2026-11-01T00:00:00Z",
        threadSubject: "Weekly Digest",
        threadSender: "digest@example.com",
        threadLastReceivedAt: "2026-11-01T00:00:00Z",
        threadSnippet: "Top stories",
        threadUnread: true,
        threadStarred: false,
        kind: "archive_with_summary",
        state: "pending",
        headline: "Digest summary",
        reason: "Informational read from 10 days ago.",
        payload: { summary: "• Key point", noteTitle: "Weekly Digest", suggestSaveToNote: true },
        evidence: [],
        createdAt: 100,
        updatedAt: 100,
      },
    ],
  };
}

describe("useInboxAgents", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.restoreAllMocks());

  it("loads snapshot, tracks pendingCount, resolves proposals, and filters by thread", async () => {
    vi.spyOn(mailClient, "getInboxAgentsSnapshot").mockResolvedValue(sampleSnapshot());
    vi.spyOn(mailClient, "syncInboxAgentAiConfig").mockResolvedValue();
    vi.spyOn(mailClient, "resolveInboxAgentProposal").mockResolvedValue();

    const { result } = renderHook(() => useInboxAgents(null, false));
    await waitFor(() => expect(result.current.loaded).toBe(true));

    expect(result.current.pendingCount).toBe(1);
    expect(result.current.proposalsForThread("thread-1")).toHaveLength(1);
    expect(result.current.proposalsForThread("other")).toHaveLength(0);

    await act(async () => {
      await result.current.resolveProposal("prop-1", "dismissed");
    });
    expect(result.current.pendingCount).toBe(0);
    expect(mailClient.resolveInboxAgentProposal).toHaveBeenCalledWith("prop-1", "dismissed");
  });

  it("toggles runner and runs an assistant", async () => {
    const snap = sampleSnapshot();
    vi.spyOn(mailClient, "getInboxAgentsSnapshot").mockResolvedValue(snap);
    vi.spyOn(mailClient, "syncInboxAgentAiConfig").mockResolvedValue();
    const toggle = vi.spyOn(mailClient, "setInboxAgentRunnerEnabled").mockResolvedValue({ ...snap, runnerEnabled: false });
    const run = vi.spyOn(mailClient, "runInboxAgent").mockResolvedValue(snap);

    const { result } = renderHook(() => useInboxAgents("you@example.com", false));
    await waitFor(() => expect(result.current.loaded).toBe(true));

    await act(async () => {
      await result.current.toggleRunner(false);
    });
    expect(toggle).toHaveBeenCalledWith(false);
    expect(result.current.snapshot.runnerEnabled).toBe(false);

    await act(async () => {
      await result.current.runAgent("stale_reads");
    });
    expect(run).toHaveBeenCalledWith("stale_reads", null, "you@example.com");
  });
});
