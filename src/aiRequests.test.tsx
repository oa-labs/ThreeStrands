import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import { AiCancelButton, AiRequestControls } from "./AiRequestControls";
import { invokeAi } from "./aiRequests";
import { saveAiCatalogPrices, saveAiPrice } from "./aiSettings";

describe("native AI request controls", () => {
  beforeEach(() => { localStorage.clear(); vi.mocked(invoke).mockReset(); Object.defineProperty(window,"__TAURI_INTERNALS__",{value:{},configurable:true}); });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });
  it("keeps the native promise attached while cancellation is acknowledged and sends only the matching request ID", async () => {
    let rejectRequest!: (error: Error) => void;
    vi.mocked(invoke).mockImplementation((command) => command === "ai_generate_reply" ? new Promise((_,reject) => { rejectRequest=reject; }) : Promise.resolve());
    saveAiPrice("openai","model",{inputPerMillion:2,outputPerMillion:8});
    const result = invokeAi("ai_generate_reply",{provider:"openai",model:"model",instruction:"private instruction"}, "thread-A").catch((error: unknown) => error);
    render(<AiCancelButton command="ai_generate_reply" owner="thread-A" />);
    const call = vi.mocked(invoke).mock.calls.find(([command]) => command === "ai_generate_reply")![1] as {control:{requestId:string;price:unknown}};
    expect(call.control.requestId).toMatch(/^[0-9a-f-]{36}$/);
    expect(call.control.price).toEqual({inputPerMillion:2,outputPerMillion:8});
    fireEvent.click(screen.getByRole("button",{name:"Cancel AI Request"}));
    expect(invoke).toHaveBeenCalledWith("cancel_ai_request",{requestId:call.control.requestId});
    // Acknowledging cancellation is not a synthetic timeout of the original call.
    expect(screen.getByRole("button",{name:"Cancel AI Request"})).toBeInTheDocument();
    await act(async () => { rejectRequest(new Error("AI request cancelled")); await result; });
    expect(screen.queryByRole("button",{name:"Cancel AI Request"})).not.toBeInTheDocument();
  });
  it("sends the catalog price when the user entered none, and the user's price over it", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    saveAiCatalogPrices("straitly", [{ id: "m", label: null, price: { inputPerMillion: 3, outputPerMillion: 15 } }]);
    const priceOf = () => (vi.mocked(invoke).mock.calls.at(-1)![1] as { control: { price: unknown } }).control.price;
    await invokeAi("ai_generate_reply", { provider: "straitly", model: "m" });
    expect(priceOf()).toEqual({ inputPerMillion: 3, outputPerMillion: 15 });
    saveAiPrice("straitly", "m", { inputPerMillion: 1, outputPerMillion: 2 });
    await invokeAi("ai_generate_reply", { provider: "straitly", model: "m" });
    expect(priceOf()).toEqual({ inputPerMillion: 1, outputPerMillion: 2 });
    await invokeAi("ai_generate_reply", { provider: "straitly", model: "unlisted" });
    expect(priceOf()).toBeNull();
  });
  it.each([
    ["ai_brief_thread", {threadId:"A"}],
    ["ai_thread_chat", {request:{threadId:"A"}}],
    ["ai_enrich_contact", {id:"A"}],
    ["ai_find_times", {request:{sourceMessageId:"A"}}],
  ])("keeps cancellation scoped to its owner for %s", async (command,args) => {
    const releases: (()=>void)[]=[];
    vi.mocked(invoke).mockImplementation(() => new Promise<void>((resolve) => releases.push(resolve)));
    const a=invokeAi(command,args);
    const view=render(<AiCancelButton command={command} owner="B" />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    view.rerender(<AiCancelButton command={command} owner="A" />);
    expect(screen.getByRole("button",{name:"Cancel AI Request"})).toBeInTheDocument();
    await act(async()=>{releases[0]();await a;});
  });
  it("requests history only while expanded without resetting an edited budget", async () => {
    const snapshot={policy:{dailyBudgetUsd:null},day:"day",chargedUsd:0,active:[],records:[],maxConcurrent:2,maxPending:8};
    vi.mocked(invoke).mockResolvedValue(snapshot);
    render(<AiRequestControls />);
    await act(async()=>{});
    expect(invoke).toHaveBeenCalledWith("ai_request_snapshot",{includeHistory:false});
    fireEvent.change(screen.getByRole("spinbutton"),{target:{value:"2"}});
    const details=screen.getByText("Request Provenance").parentElement as HTMLDetailsElement;
    await act(async()=>{details.open=true;fireEvent(details,new Event("toggle"));});
    expect(invoke).toHaveBeenCalledWith("ai_request_snapshot",{includeHistory:true});
    expect(screen.getByRole("spinbutton")).toHaveValue(2);
    await act(async()=>{details.open=false;fireEvent(details,new Event("toggle"));});
    expect(invoke).toHaveBeenLastCalledWith("ai_request_snapshot",{includeHistory:false});
  });
  it("requires an explicit recovery action after controls fail to load", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("unavailable"));
    render(<AiRequestControls />);
    await screen.findByRole("alert");
    expect(vi.mocked(invoke).mock.calls.some(([command])=>command==="recover_ai_requests")).toBe(false);
    const details=screen.getByText("Recover AI Controls").parentElement as HTMLDetailsElement;
    await act(async()=>{details.open=true;fireEvent(details,new Event("toggle"));});
    vi.mocked(invoke).mockResolvedValue({policy:{dailyBudgetUsd:null},chargedUsd:0,active:[],records:[],maxConcurrent:2,maxPending:8});
    await act(async()=>{fireEvent.click(screen.getByRole("button",{name:"Reset Controls and Unknown Reservations"}));});
    expect(invoke).toHaveBeenCalledWith("recover_ai_requests");
  });
  it("saves a native device-local budget and preserves entered text during polling", async () => {
    const snapshot = {policy:{dailyBudgetUsd:null},day:"2026-11-16",chargedUsd:0.1,active:[],records:[],maxConcurrent:2,maxPending:8};
    vi.mocked(invoke).mockImplementation((command,args) => {
      if(command==="set_ai_request_policy") { snapshot.policy=(args as {policy:typeof snapshot.policy}).policy; return Promise.resolve(); }
      return Promise.resolve(snapshot);
    });
    render(<AiRequestControls />);
    const save = await screen.findByRole("button",{name:"Save Budget"});
    const input = screen.getByRole("spinbutton",{name:"Daily AI Budget (USD)"});
    expect(input).toHaveAccessibleDescription();
    fireEvent.change(input,{target:{value:"1.25"}});
    await act(async () => { fireEvent.click(save); });
    expect(invoke).toHaveBeenCalledWith("set_ai_request_policy",{policy:{dailyBudgetUsd:1.25}});
    expect(input).toHaveValue(1.25);
    fireEvent.change(input,{target:{value:""}});
    await act(async () => { fireEvent.click(save); });
    expect(invoke).toHaveBeenCalledWith("set_ai_request_policy",{policy:{dailyBudgetUsd:null}});
  });
});
