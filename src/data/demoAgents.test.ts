import { describe, expect, it } from "vitest";
import { createDemoClient } from "./demoClient";
import { DEMO_ACCOUNT_ID, defaultDemoDataset } from "./demoDataset";

describe("delegated fixture work client", () => {
  it("is disabled until explicitly enabled and rejects live workflows", async () => {
    const client = createDemoClient(defaultDemoDataset());
    expect(await client.agentRunnerEnabled()).toBe(false);
    const request = { accountId: DEMO_ACCOUNT_ID, objective: "Evaluate invitation", workflow: "fixture_research_v1" as const };
    await expect(client.createAgentTask(request)).rejects.toThrow("disabled");
    await client.setAgentRunnerEnabled(true);
    await expect(client.createAgentTask({ ...request, accountId: "other@example.com" })).rejects.toThrow("Account");
    await expect(client.createAgentTask({ ...request, linkedTaskId: "unknown" })).rejects.toThrow("Linked task");
  });
  it("waits for clarification, answers once, and keeps personal task completion independent", async () => {
    const client = createDemoClient(defaultDemoDataset());
    await client.setAgentRunnerEnabled(true);
    const tasks = await client.listTasks();
    const d = await client.createAgentTask({ accountId: DEMO_ACCOUNT_ID, objective: "Evaluate invitation", workflow: "fixture_research_v1", linkedTaskId: tasks[0]?.id });
    const waiting = await client.getAgentTask(d.task.id);
    expect(waiting.run.state).toBe("waiting_for_input");
    const question = waiting.run.questionId!;
    const done = await client.answerAgentQuestion(d.task.id, question, "Customer introductions");
    expect(done.run.state).toBe("completed");
    expect(done.artifacts).toHaveLength(1);
    expect(done.artifacts[0].notResearched).toBe(true);
    expect(done.artifacts[0].body).toContain("No external research was performed");
    const duplicate = await client.answerAgentQuestion(d.task.id, question, "Customer introductions");
    expect(duplicate.turns).toHaveLength(done.turns.length);
    await expect(client.answerAgentQuestion(d.task.id, question, "Different")).rejects.toThrow("already answered");
    expect(await client.listTasks()).toEqual(tasks);
    done.task.objective = "External mutation";
    expect((await client.getAgentTask(d.task.id)).task.objective).toBe("Evaluate invitation");
  });
  it("supports pause, disabled resume, archive/read, cancellation and deletion", async () => {
    const client = createDemoClient(defaultDemoDataset());
    await client.setAgentRunnerEnabled(true);
    const d = await client.createAgentTask({ accountId: DEMO_ACCOUNT_ID, objective: "Evaluate invitation", workflow: "fixture_research_v1" });
    await client.controlAgentTask(d.task.id, "pause");
    await client.setAgentRunnerEnabled(false);
    await expect(client.controlAgentTask(d.task.id, "resume")).rejects.toThrow("disabled");
    await client.setAgentRunnerEnabled(true);
    expect((await client.controlAgentTask(d.task.id, "resume")).run.state).toBe("waiting_for_input");
    await client.controlAgentTask(d.task.id, "mark_read");
    await client.controlAgentTask(d.task.id, "archive");
    expect(await client.listAgentTasks()).toHaveLength(0);
    expect(await client.listAgentTasks(undefined, true)).toHaveLength(1);
    const cancelled = await client.controlAgentTask(d.task.id, "cancel");
    await expect(client.answerAgentQuestion(d.task.id, cancelled.run.questionId!, "Sales")).rejects.toThrow("not pending");
    expect(cancelled.artifacts).toHaveLength(0);
    await client.deleteAgentTask(d.task.id);
    await client.deleteAgentTask(d.task.id);
    await expect(client.getAgentTask(d.task.id)).rejects.toThrow("not found");
  });
});
