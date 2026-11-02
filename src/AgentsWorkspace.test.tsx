import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { AgentsWorkspace } from "./AgentsWorkspace";
import { useAgentTasks } from "./useAgentTasks";
import { createDemoClient } from "./data/demoClient";
import { defaultDemoDataset, DEMO_ACCOUNT_ID } from "./data/demoDataset";
import { mailClient } from "./data/client";
import type { AgentDetail } from "./domain";

const dataset = defaultDemoDataset();
const openThread = vi.fn();
const openTask = vi.fn();
function Harness() {
  const c = useAgentTasks();
  const [shown, show] = useState(true);
  return <><button onClick={() => show(!shown)}>Switch workspace</button>{shown ? <AgentsWorkspace controller={c} accounts={dataset.accounts} accountId={null} onOpenThread={openThread} onOpenTask={openTask} /> : <p>Another workspace</p>}</>;
}
async function start() {
  fireEvent.click(await screen.findByRole("button", { name: "New delegated task" }));
  fireEvent.click(screen.getByRole("button", { name: "Enable local demonstration" }));
  await waitFor(() => expect(screen.getByRole("checkbox", { name: "Local demonstration enabled" })).toBeChecked());
  fireEvent.change(screen.getByRole("textbox", { name: "What would you like to find out?" }), { target: { value: "Evaluate the podcast invitation" } });
  fireEvent.click(screen.getByRole("button", { name: "Start demonstration" }));
  await screen.findByRole("article", { name: "Delegated task details" });
  fireEvent.click(screen.getByRole("button", { name: "Refresh agents" }));
  const tab = screen.queryByRole("tab", { name: "Conversation" }); if (tab) fireEvent.click(tab);
  await screen.findByRole("textbox", { name: "Your answer" });
}
describe("delegated workspace", () => {
  beforeEach(() => {
    const client = createDemoClient(dataset);
    for (const key of ["agentRunnerEnabled", "setAgentRunnerEnabled", "createAgentTask", "getAgentTask", "listAgentTasks", "controlAgentTask", "deleteAgentTask", "answerAgentQuestion", "interactAgentTask", "linkAgentTask", "markAgentTaskRead", "listTasks", "getThread"] as const) vi.spyOn(mailClient, key).mockImplementation(client[key] as never);
    openThread.mockReset(); openTask.mockReset();
  });
  afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

  it("delegates, leaves, preserves an answer draft, returns, and refines immutable reports", async () => {
    render(<Harness />); await start();
    expect(screen.getAllByText("Answer needed")).toHaveLength(1);
    const answer = screen.getByRole("textbox", { name: "Your answer" });
    fireEvent.change(answer, { target: { value: "Prioritize customer introductions" } });
    fireEvent.click(screen.getByRole("button", { name: "Switch workspace" }));
    fireEvent.click(screen.getByRole("button", { name: "Switch workspace" }));
    expect(screen.getByRole("textbox", { name: "Your answer" })).toHaveValue("Prioritize customer introductions");
    await waitFor(() => expect(screen.getByRole("button", { name: "Answer question" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "Answer question" }));
    await screen.findByText("Demonstration report · Not researched");
    expect(screen.queryByText("Answer needed")).not.toBeInTheDocument();
    const report = screen.getByRole("region", { name: "Agent result" });
    const original = within(report).getByText(/Fixture report\./).textContent;
    fireEvent.change(screen.getByRole("textbox", { name: "Follow-up message" }), { target: { value: "Why is this only a demonstration?" } });
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    await screen.findByText(/This is a demonstration response/);
    expect(within(report).getByText(/Fixture report\./).textContent).toBe(original);
    await waitFor(() => expect(screen.getByRole("button", { name: "Send message" })).toBeDisabled());
    fireEvent.change(screen.getByRole("textbox", { name: "Follow-up message" }), { target: { value: "Focus on sales <img src=x onerror=alert(1)>" } });
    fireEvent.click(screen.getByRole("button", { name: "Revise report" }));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Report version" }).querySelectorAll("option")).toHaveLength(2));
    expect(within(report).getByText(/Revision request:/)).toHaveTextContent("Focus on sales <img src=x onerror=alert(1)>");
    expect(report.querySelector("img")).toBeNull();
    const options = screen.getByRole("combobox", { name: "Report version" }).querySelectorAll("option");
    fireEvent.change(screen.getByRole("combobox", { name: "Report version" }), { target: { value: options[0].value } });
    expect(within(report).getByText(/Fixture report\./).textContent).toBe(original);
  });

  it("explains unsupported research, keeps drafts after failure, and requires deliberate deletion", async () => {
    render(<Harness />);
    fireEvent.click(await screen.findByRole("button", { name: "New delegated task" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Work" }), { target: { value: "web" } });
    expect(screen.getByRole("button", { name: "Start demonstration" })).toBeDisabled();
    expect(screen.getByRole("status")).toHaveTextContent("Web research is not available yet");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" })); await start();
    vi.mocked(mailClient.answerAgentQuestion).mockRejectedValueOnce(new Error("Offline test failure"));
    fireEvent.change(screen.getByRole("textbox", { name: "Your answer" }), { target: { value: "Sales" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Answer question" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "Answer question" }));
    await screen.findByRole("alert"); expect(screen.getByRole("textbox", { name: "Your answer" })).toHaveValue("Sales");
    fireEvent.click(screen.getByRole("button", { name: "Delete delegated task" }));
    expect(mailClient.deleteAgentTask).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Delete permanently" }));
    await waitFor(() => expect(screen.queryByRole("article")).toBeNull());
  });

  it("uses accessible tabs in a narrow view and retains unsent input", async () => {
    vi.stubGlobal("matchMedia", () => ({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
    render(<Harness />); await start();
    fireEvent.click(screen.getByRole("tab", { name: "Conversation" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Your answer" }), { target: { value: "Keep this draft" } });
    fireEvent.click(screen.getByRole("tab", { name: "Result" }));
    expect(screen.queryByRole("textbox", { name: "Your answer" })).toBeNull();
    fireEvent.click(screen.getByRole("tab", { name: "Conversation" }));
    expect(screen.getByRole("textbox", { name: "Your answer" })).toHaveValue("Keep this draft");
    vi.unstubAllGlobals();
  });

  it("renders detached sources, empty results, and recoverable failure without losing the previous report", async () => {
    const d: AgentDetail = { task: { id: "failed", accountId: DEMO_ACCOUNT_ID, objective: "Failed revision", workflow: "fixture_research_v1", revision: 2, createdAt: 0, sourceThreadId: null, linkedTaskId: null, unread: false, archived: false }, run: { id: "r", taskId: "failed", state: "failed", reason: "retry_exhausted", generation: 2, checkpoint: 2, owner: null, leaseUntil: null, startedAt: null, notBefore: 0, dispatches: 3, attempts: 3, activeSeconds: 0, questionId: null }, turns: [], artifacts: [{ id: "a1", version: 1, kind: "fixture_report", body: "Previous completed result", notResearched: true, createdAt: 0 }] };
    vi.mocked(mailClient.listAgentTasks).mockImplementation(async () => structuredClone([d])); vi.mocked(mailClient.agentRunnerEnabled).mockResolvedValue(true);
    render(<Harness />);
    fireEvent.click(await screen.findByRole("button", { name: /Failed revision/ }));
    expect(screen.getByText("No linked conversation is available.")).toBeInTheDocument();
    expect(screen.getByText("Previous completed result")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Retry$/ })).toBeEnabled();
    d.run.state = "completed"; d.artifacts = [];
    fireEvent.click(screen.getByRole("button", { name: "Refresh agents" }));
    await screen.findByText("This task finished without a report.");
  });
});
