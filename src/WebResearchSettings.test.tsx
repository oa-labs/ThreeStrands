import frozenPolicy from "../docs/agent-integrations/contract-v1.json";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WebResearchSettings } from "./WebResearchSettings";
import { mailClient } from "./data/client";
import type { AgentResearchSettings } from "./domain";

const blank: AgentResearchSettings = {
  policy: { ...frozenPolicy, limits: { ...frozenPolicy.limits, searchCallsPerTask: 7 } },
  config: { enabled: false, searchEnabled: false, persistenceBasis: "", verified: false },
  modelKeyConfigured: false,
  searchKeyConfigured: false,
};
const configured = (config: Partial<AgentResearchSettings["config"]> = {}): AgentResearchSettings => ({
  ...blank,
  config: { ...blank.config, searchEnabled: true, persistenceBasis: "Our API agreement", ...config },
  modelKeyConfigured: true,
  searchKeyConfigured: true,
});

afterEach(() => { vi.restoreAllMocks(); localStorage.clear(); });

describe("web research settings", () => {
  it("saves keys only through native keychain commands and keeps a key that failed to save", async () => {
    const read = vi.spyOn(mailClient, "agentResearchSettings").mockResolvedValue(blank);
    const key = vi.spyOn(mailClient, "setAgentResearchKey").mockRejectedValueOnce(new Error("Keychain locked"));
    const onChange = vi.fn();
    render(<WebResearchSettings onChange={onChange} />);
    const input = await screen.findByLabelText("Anthropic API Key");
    fireEvent.change(input, { target: { value: "temporary-test-secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Anthropic API Key" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Keychain locked");
    expect(input).toHaveValue("temporary-test-secret");
    expect(onChange).not.toHaveBeenCalled();

    key.mockResolvedValue();
    read.mockResolvedValue({ ...blank, modelKeyConfigured: true });
    fireEvent.click(screen.getByRole("button", { name: "Save Anthropic API Key" }));
    await waitFor(() => expect(input).toHaveValue(""));
    expect(key).toHaveBeenLastCalledWith("model", "temporary-test-secret");
    expect(onChange).toHaveBeenCalledOnce();
    expect(await screen.findByRole("button", { name: "Remove Anthropic API Key" })).toBeEnabled();

    fireEvent.change(screen.getByLabelText("Exa API Key"), { target: { value: "temporary-search-secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Exa API Key" }));
    await waitFor(() => expect(key).toHaveBeenLastCalledWith("search", "temporary-search-secret"));
    expect(JSON.stringify({ ...localStorage })).not.toContain("temporary");
  });

  it("removes a saved key through the keychain command", async () => {
    vi.spyOn(mailClient, "agentResearchSettings").mockResolvedValue(configured({ verified: true, enabled: true }));
    const key = vi.spyOn(mailClient, "setAgentResearchKey").mockResolvedValue();
    render(<WebResearchSettings />);
    fireEvent.click(await screen.findByRole("button", { name: "Remove Exa API Key" }));
    await waitFor(() => expect(key).toHaveBeenCalledWith("search", ""));
  });

  it("names what is missing and allows the paid check only once keys and agreement are saved", async () => {
    vi.spyOn(mailClient, "agentResearchSettings").mockResolvedValue({ ...blank, modelKeyConfigured: true });
    const check = vi.spyOn(mailClient, "checkAgentResearchConnection").mockResolvedValue();
    render(<WebResearchSettings />);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("add an Exa API key and your storage agreement"));
    const button = screen.getByRole("button", { name: "Check Connections" });
    expect(button).toBeDisabled();
    expect(screen.getByText(/one public test search to Exa, which may cost \$0\.007/)).toBeVisible();
    expect(check).not.toHaveBeenCalled();
  });

  it("checks connections, then reports research as ready only when it is turned on", async () => {
    const read = vi.spyOn(mailClient, "agentResearchSettings").mockResolvedValue(configured());
    const check = vi.spyOn(mailClient, "checkAgentResearchConnection").mockResolvedValue();
    render(<WebResearchSettings />);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Check connections to finish setup."));
    read.mockResolvedValue(configured({ verified: true }));
    fireEvent.click(screen.getByRole("button", { name: "Check Connections" }));
    await waitFor(() => expect(check).toHaveBeenCalledOnce());
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Turn on Web Research to use it."));
    read.mockResolvedValue(configured({ verified: true, enabled: true }));
    vi.spyOn(mailClient, "setAgentResearchSettings").mockResolvedValue();
    fireEvent.click(screen.getByRole("checkbox", { name: "Web Research" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Ready."));
  });

  it("turns research on with Exa search, which web research requires, and never claims verification", async () => {
    vi.spyOn(mailClient, "agentResearchSettings").mockResolvedValue(blank);
    const save = vi.spyOn(mailClient, "setAgentResearchSettings").mockResolvedValue();
    render(<WebResearchSettings />);
    const toggle = await screen.findByRole("checkbox", { name: "Web Research" });
    expect(toggle).toHaveAccessibleDescription();
    fireEvent.click(toggle);
    await waitFor(() => expect(save).toHaveBeenCalledWith({ enabled: true, searchEnabled: true, persistenceBasis: "", verified: false }));
    expect(screen.queryByRole("checkbox", { name: /Exa/ })).not.toBeInTheDocument();
  });

  it("saves the storage agreement on request and keeps an unsaved draft after a failure", async () => {
    vi.spyOn(mailClient, "agentResearchSettings").mockResolvedValue(blank);
    const save = vi.spyOn(mailClient, "setAgentResearchSettings").mockRejectedValueOnce(new Error("Save failed"));
    render(<WebResearchSettings />);
    const agreement = await screen.findByRole("textbox", { name: "Storage Agreement" });
    expect(agreement).toHaveAccessibleDescription();
    const saveButton = screen.getByRole("button", { name: "Save Agreement" });
    expect(saveButton).toBeDisabled();
    fireEvent.change(agreement, { target: { value: "Our API agreement permits saved excerpts" } });
    expect(save).not.toHaveBeenCalled();
    fireEvent.click(saveButton);
    expect(await screen.findByRole("alert")).toHaveTextContent("Save failed");
    expect(agreement).toHaveValue("Our API agreement permits saved excerpts");
    save.mockResolvedValue();
    fireEvent.click(saveButton);
    await waitFor(() => expect(save).toHaveBeenLastCalledWith(expect.objectContaining({
      persistenceBasis: "Our API agreement permits saved excerpts", searchEnabled: true, verified: false,
    })));
  });

  it("shows the per-task limits from the native policy", async () => {
    vi.spyOn(mailClient, "agentResearchSettings").mockResolvedValue(blank);
    render(<WebResearchSettings />);
    const searches = await screen.findByText("Searches");
    expect(searches.nextElementSibling).toHaveTextContent("7");
    expect(screen.getByText(/No exact dollar cap is promised/)).toBeInTheDocument();
    expect(screen.getByText(/this is not a zero-retention promise/)).toBeInTheDocument();
  });
});
