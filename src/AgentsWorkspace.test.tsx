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
const openResearchSettings = vi.fn();
function Harness() {
  const c = useAgentTasks();
  const [shown, show] = useState(true);
  return <><button onClick={() => show(!shown)}>Switch workspace</button>{shown ? <AgentsWorkspace controller={c} accounts={dataset.accounts} accountId={null} onOpenThread={openThread} onOpenTask={openTask} onOpenResearchSettings={openResearchSettings} /> : <p>Another workspace</p>}</>;
}
async function start() {
  fireEvent.click(await screen.findByRole("button", { name: "New delegated task" }));
  fireEvent.click(screen.getByRole("button", { name: "Enable local demonstration" }));
  await waitFor(() => expect(screen.getByRole("checkbox", { name: "Delegated work enabled" })).toBeChecked());
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
    expect(screen.getByRole("button", { name: "Start research" })).toBeDisabled();
    expect(screen.getByRole("status")).toHaveTextContent("Web research is not configured");
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

describe("real research entry",()=> {
  afterEach(()=>vi.restoreAllMocks());
  it("requires reviewed public subjects and consent before sending an objective to native research",async()=> {
    const client=createDemoClient(dataset);
    await client.setAgentRunnerEnabled(true);
    for(const key of ["agentRunnerEnabled","listAgentTasks","listTasks"] as const) vi.spyOn(mailClient,key).mockImplementation(client[key] as never);
    vi.spyOn(mailClient,"agentResearchSettings").mockResolvedValue({config:{enabled:true,searchEnabled:true,persistenceBasis:"Applicable API agreement",verified:true},modelKeyConfigured:true,searchKeyConfigured:true});
    const d=await client.createAgentTask({accountId:DEMO_ACCOUNT_ID,objective:"Check the show",workflow:"fixture_research_v1"});
    const create=vi.spyOn(mailClient,"createAgentResearchTask").mockResolvedValue({...d,task:{...d.task,workflow:"web_research_v1"}});
    render(<Harness />);
    fireEvent.click(await screen.findByRole("button",{name:"New delegated task"}));
    fireEvent.change(screen.getByRole("combobox",{name:"Work"}),{target:{value:"web"}});
    fireEvent.change(screen.getByRole("textbox",{name:"What would you like to find out?"}),{target:{value:"Assess audience fit"}});
    expect(screen.getByText(/Your objective, public subjects, follow-up messages, and retrieved evidence go to Anthropic/)).toBeVisible();
    const start=screen.getByRole("button",{name:"Start research"});expect(start).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox",{name:"Public search subjects (one per line)"}),{target:{value:"Example Podcast\nExample Host"}});
    expect(start).toBeDisabled();expect(create).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("checkbox",{name:/I reviewed these public subjects/}));
    await waitFor(()=>expect(start).toBeEnabled());fireEvent.click(start);
    await waitFor(()=>expect(create).toHaveBeenCalledWith({task:expect.objectContaining({workflow:"web_research_v1",objective:"Assess audience fit"}),subjects:["Example Podcast","Example Host"],consent:true}));
  });
  it("sends research setup to Settings and re-reads it when Settings closes",async()=> {
    const client=createDemoClient(dataset);
    for(const key of ["agentRunnerEnabled","listAgentTasks","listTasks"] as const) vi.spyOn(mailClient,key).mockImplementation(client[key] as never);
    const read=vi.spyOn(mailClient,"agentResearchSettings").mockResolvedValue({config:{enabled:false,searchEnabled:true,persistenceBasis:"",verified:false},modelKeyConfigured:false,searchKeyConfigured:false});
    function Workspace({settingsOpen}:{settingsOpen:boolean}) {const c=useAgentTasks();return <AgentsWorkspace controller={c} accounts={dataset.accounts} accountId={null} onOpenThread={openThread} onOpenTask={openTask} onOpenResearchSettings={openResearchSettings} settingsOpen={settingsOpen} />;}
    openResearchSettings.mockReset();
    const view=render(<Workspace settingsOpen={false} />);
    fireEvent.click(await screen.findByRole("button",{name:"Research settings"}));
    expect(openResearchSettings).toHaveBeenCalledOnce();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(()=>expect(read).toHaveBeenCalledTimes(1));
    view.rerender(<Workspace settingsOpen />);
    view.rerender(<Workspace settingsOpen={false} />);
    await waitFor(()=>expect(read).toHaveBeenCalledTimes(3));
  });
  it("keeps each report's retrieved sources separate and opens only native source IDs",async()=> {
    const client=createDemoClient(dataset);await client.setAgentRunnerEnabled(true);
    const d=await client.createAgentTask({accountId:DEMO_ACCOUNT_ID,objective:"Review sourced reports",workflow:"fixture_research_v1"});
    d.task.workflow="web_research_v1";d.run.state="completed";d.run.questionId=null;
    d.artifacts=[{id:"v1",version:1,kind:"research_report",body:"Old <img src=x onerror=alert(1)> report",notResearched:false,createdAt:1},{id:"v2",version:2,kind:"research_report",body:"Second report",notResearched:false,createdAt:2}];
    d.research={subjects:["Public Show"],usage:{modelCalls:2,countCalls:2,searchCalls:1,pageCalls:1,reservedInput:100,reservedOutput:100,inputTokens:100,outputTokens:100,unknownUsage:0,unknownSearchCost:0,searchCostUsd:null},citations:{v1:["s1"],v2:["s2"]},sources:[{id:"s1",url:"https://example.com/first",title:"Old source",excerpt:"First evidence",kind:"search_snippet",retrievedAt:1},{id:"s2",url:"https://example.net/second",title:"New source",excerpt:"Second evidence",kind:"page",retrievedAt:2}]};
    vi.spyOn(mailClient,"agentRunnerEnabled").mockResolvedValue(true);vi.spyOn(mailClient,"listAgentTasks").mockResolvedValue([d]);vi.spyOn(mailClient,"listTasks").mockResolvedValue([]);
    const open=vi.spyOn(mailClient,"openAgentResearchSource").mockResolvedValue();
    render(<Harness />);fireEvent.click(await screen.findByRole("button",{name:/Review sourced reports/}));
    expect(screen.getByRole("button",{name:"New source"})).toBeVisible();expect(screen.queryByRole("button",{name:"Old source"})).toBeNull();expect(open).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("combobox",{name:"Report version"}),{target:{value:"v1"}});
    expect(screen.queryByRole("button",{name:"New source"})).toBeNull();expect(screen.getByText("Old <img src=x onerror=alert(1)> report")).toBeVisible();expect(document.querySelector(".agent-report-body img")).toBeNull();
    fireEvent.click(screen.getByRole("button",{name:"Old source"}));expect(open).toHaveBeenCalledWith(d.task.id,"s1");
  });

});
