import frozenPolicy from "../docs/agent-integrations/contract-v1.json";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentResearchSettings } from "./AgentResearchSettings";
import { mailClient } from "./data/client";

afterEach(()=>vi.restoreAllMocks());
describe("research setup",()=> {
  it("saves credentials only through native keychain commands, discloses a paid check, and keeps failed input",async()=> {
    vi.spyOn(mailClient,"agentResearchSettings").mockResolvedValue({policy:{...frozenPolicy,limits:{...frozenPolicy.limits,searchCallsPerTask:7}},config:{enabled:false,searchEnabled:false,persistenceBasis:"",verified:false},modelKeyConfigured:false,searchKeyConfigured:false});
    const save=vi.spyOn(mailClient,"setAgentResearchSettings").mockRejectedValueOnce(new Error("Save failed"));
    const key=vi.spyOn(mailClient,"setAgentResearchKey").mockResolvedValue();
    const check=vi.spyOn(mailClient,"checkAgentResearchConnection").mockResolvedValue();
    render(<AgentResearchSettings onClose={vi.fn()} onSaved={vi.fn()} />);
    const input=await screen.findByLabelText("Anthropic API key");
    expect(screen.getByText(/7 searches/)).toBeVisible();
    fireEvent.change(input,{target:{value:"temporary-test-secret"}});
    fireEvent.change(screen.getByLabelText("Exa API key"),{target:{value:"temporary-search-secret"}});
    fireEvent.change(screen.getByLabelText("Agreement basis for saving Exa results"),{target:{value:"Our API agreement permits saved excerpts"}});
    expect(screen.getByLabelText("Agreement basis for saving Exa results")).toHaveAccessibleDescription();
    expect(screen.getByRole("button",{name:"Check connections"})).toBeDisabled();
    expect(check).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button",{name:"Save research settings"}));
    await screen.findByText("Save failed");expect(input).toHaveValue("temporary-test-secret");expect(key).not.toHaveBeenCalled();
    save.mockResolvedValue();
    fireEvent.click(screen.getByRole("button",{name:"Save research settings"}));
    await waitFor(()=>expect(key).toHaveBeenCalledWith("search","temporary-search-secret"));
    expect(key).toHaveBeenCalledWith("model","temporary-test-secret");
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({persistenceBasis:"Our API agreement permits saved excerpts",verified:false}));
    expect(localStorage.getItem("agents-anthropic-api-key")).toBeNull();
    expect(check).not.toHaveBeenCalled();
  });
});
