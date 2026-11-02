import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AGENT_REFRESH_MS, useAgentTasks } from "./useAgentTasks";
import { mailClient } from "./data/client";
import { createDemoClient } from "./data/demoClient";
import { defaultDemoDataset, DEMO_ACCOUNT_ID } from "./data/demoDataset";

async function fixture() {
  const client = createDemoClient(defaultDemoDataset());
  await client.setAgentRunnerEnabled(true);
  const d = await client.createAgentTask({ accountId: DEMO_ACCOUNT_ID, objective: "Fixture job", workflow: "fixture_research_v1" });
  return client.getAgentTask(d.task.id);
}
describe("agent workspace data lifecycle", () => {
  beforeEach(() => {
    vi.spyOn(mailClient, "agentRunnerEnabled").mockResolvedValue(true);
    vi.spyOn(mailClient, "listAgentTasks").mockResolvedValue([]);
  });
  afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });
  it("rejects a stale list response after deleting work", async () => {
    const d = await fixture();
    vi.mocked(mailClient.listAgentTasks).mockResolvedValueOnce([d]);
    const { result } = renderHook(() => useAgentTasks());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    let resolve!: (value: typeof d[]) => void;
    vi.mocked(mailClient.listAgentTasks).mockImplementationOnce(() => new Promise(r => { resolve = r; }));
    let pending!: Promise<void>;
    act(() => { pending = result.current.refresh(); });
    vi.spyOn(mailClient, "deleteAgentTask").mockResolvedValue();
    await act(async () => { await result.current.remove(d.task.id); });
    await act(async () => { resolve([d]); await pending; });
    expect(result.current.items).toHaveLength(0);
  });
  it("announces background completion without changing selection or drafts", async () => {
    const d = await fixture(); d.run.state = "queued";
    vi.mocked(mailClient.listAgentTasks).mockImplementation(async () => structuredClone([d]));
    const { result } = renderHook(() => useAgentTasks());
    await waitFor(() => expect(result.current.loaded).toBe(true));
    act(() => { result.current.patchView("other", { draft: "Keep typing" }); result.current.select("other"); });
    d.run.state = "completed"; d.task.unread = true;
    await act(async () => { await result.current.refresh(); });
    expect(result.current.activity?.label).toMatch(/Result ready/);
    expect(result.current.selectedId).toBe("other"); expect(result.current.view("other").draft).toBe("Keep typing");
  });
  it("polls existing running jobs without enqueueing or resuming work", async () => {
    const d = await fixture(); d.run.state = "running";
    vi.mocked(mailClient.listAgentTasks).mockResolvedValue([d]);
    const create = vi.spyOn(mailClient, "createAgentTask"); const control = vi.spyOn(mailClient, "controlAgentTask");
    vi.useFakeTimers();
    const { result, unmount } = renderHook(() => useAgentTasks());
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(result.current.loaded).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(AGENT_REFRESH_MS); });
    expect(mailClient.listAgentTasks).toHaveBeenCalledTimes(2);
    expect(create).not.toHaveBeenCalled(); expect(control).not.toHaveBeenCalled();
    unmount();
  });
});
