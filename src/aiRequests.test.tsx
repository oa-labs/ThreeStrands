import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
import { invoke } from "@tauri-apps/api/core";
import { AiCancelButton, AiRequestControls } from "./AiRequestControls";
import { invokeAi } from "./aiRequests";
import { saveAiPrice } from "./aiSettings";

describe("native AI request controls", () => {
  beforeEach(() => { localStorage.clear(); vi.mocked(invoke).mockReset(); Object.defineProperty(window,"__TAURI_INTERNALS__",{value:{},configurable:true}); });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });
  it("keeps the native promise attached while cancellation is acknowledged and sends only the matching request ID", async () => {
    let rejectRequest!: (error: Error) => void;
    vi.mocked(invoke).mockImplementation((command) => command === "ai_generate_reply" ? new Promise((_,reject) => { rejectRequest=reject; }) : Promise.resolve());
    saveAiPrice("openai","model",{inputPerMillion:2,outputPerMillion:8});
    const result = invokeAi("ai_generate_reply",{provider:"openai",model:"model",instruction:"private instruction"}).catch((error: unknown) => error);
    render(<AiCancelButton command="ai_generate_reply" />);
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
