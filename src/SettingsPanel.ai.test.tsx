import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { readAiFastModel, readAiFeatures, readAiModel, saveAiProvider } from "./aiSettings";
import { AiProviderSettings } from "./AiProviderSettings";
import { queuePortablePreferences } from "./syncedPreferences";

describe("AI provider feature settings", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    vi.mocked(invoke).mockResolvedValue(false);
    saveAiProvider("openai");
  });
  afterEach(cleanup);

  it("saves the latest flags before queueing them for sync", async () => {
    render(<AiProviderSettings onChange={queuePortablePreferences} />);
    // The writing profile has its own Workflow section.
    expect(screen.queryByRole("textbox", { name: "About You" })).not.toBeInTheDocument();

    act(() => {
      fireEvent.click(screen.getByRole("checkbox", { name: "Writing" }));
      fireEvent.click(screen.getByRole("checkbox", { name: "Contacts" }));
    });

    await waitFor(() => expect(vi.mocked(invoke).mock.calls
      .filter(([command]) => command === "update_synced_preferences")).toHaveLength(2));
    const queued = vi.mocked(invoke).mock.calls
      .filter(([command]) => command === "update_synced_preferences");
    expect(queued).toHaveLength(2);
    expect(queued[1][1]).toEqual({
      preferences: expect.objectContaining({
        aiFeatures: expect.objectContaining({ draftAssist: false, contactEnrichment: false }),
      }),
    });
    expect(readAiFeatures()).toMatchObject({ draftAssist: false, contactEnrichment: false });
  });

  it("queues every model and endpoint edit for sync so a later pull cannot revert it", async () => {
    saveAiProvider("custom");
    render(<AiProviderSettings onChange={queuePortablePreferences} />);

    const fast = screen.getByRole("combobox", { name: "Fast model" });
    const reasoning = screen.getByRole("combobox", { name: "Reasoning model" });
    // The combobox commits on blur, not per keystroke, so each edit ends with a blur.
    fireEvent.change(fast, { target: { value: "example/fast" } });
    fireEvent.blur(fast);
    fireEvent.change(reasoning, { target: { value: "example/reasoning" } });
    fireEvent.blur(reasoning);
    fireEvent.change(screen.getByRole("textbox", { name: "Endpoint URL" }), { target: { value: "https://api.example.com/v1" } });

    await waitFor(() => expect(vi.mocked(invoke).mock.calls
      .filter(([command]) => command === "update_synced_preferences")).toHaveLength(3));
    const queued = vi.mocked(invoke).mock.calls
      .filter(([command]) => command === "update_synced_preferences");
    expect(queued[0][1]).toEqual({ preferences: expect.objectContaining({ aiFastModel: "example/fast" }) });
    expect(queued[2][1]).toEqual({
      preferences: expect.objectContaining({
        aiFastModel: "example/fast",
        aiModel: "example/reasoning",
        aiEndpoint: "https://api.example.com/v1",
      }),
    });
  });

  /** Answers the key check and model lists; `lists` maps provider to its catalog. */
  function mockCatalog(
    keyConfigured: boolean,
    lists: Record<string, unknown> = {},
  ) {
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "ai_api_key_configured") return keyConfigured;
      if (command === "ai_list_models") {
        const provider = (args as { provider: string }).provider;
        return lists[provider] ?? [];
      }
      return false;
    });
  }
  const listCalls = () => vi.mocked(invoke).mock.calls
    .filter(([command]) => command === "ai_list_models")
    .map(([, args]) => (args as { provider: string }).provider);

  it("loads the saved provider's models once, after the key check answers", async () => {
    mockCatalog(true, { openai: [{ id: "gpt-4.1", label: null }] });
    render(<AiProviderSettings />);

    await waitFor(() => expect(listCalls()).toEqual(["openai"]));
    // The key check resolving must not start a second fetch.
    await act(async () => {});
    expect(listCalls()).toEqual(["openai"]);
  });

  it("never sends the saved key to a newly selected provider's catalog on its own", async () => {
    mockCatalog(true);
    render(<AiProviderSettings />);
    await waitFor(() => expect(listCalls()).toEqual(["openai"]));
    const providerSelect = screen.getByRole("combobox", { name: "Provider" });

    fireEvent.change(providerSelect, { target: { value: "anthropic" } });
    await act(async () => {});
    expect(listCalls()).toEqual(["openai"]);

    // OpenRouter's catalog is public and fetched without the key.
    fireEvent.change(providerSelect, { target: { value: "openrouter" } });
    await waitFor(() => expect(listCalls()).toEqual(["openai", "openrouter"]));

    // A keyed provider loads only on an explicit refresh.
    fireEvent.change(providerSelect, { target: { value: "anthropic" } });
    fireEvent.click(screen.getByRole("button", { name: "Refresh model list" }));
    await waitFor(() => expect(listCalls()).toEqual(["openai", "openrouter", "anthropic"]));
  });

  it("ignores a model list that arrives after the user switched providers", async () => {
    saveAiProvider("openrouter");
    let resolveOpenRouter: (models: unknown) => void = () => {};
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "ai_api_key_configured") return true;
      if (command === "ai_list_models") {
        const provider = (args as { provider: string }).provider;
        if (provider === "openrouter") return new Promise((resolve) => { resolveOpenRouter = resolve; });
        return [{ id: "gpt-4.1", label: null }];
      }
      return false;
    });
    render(<AiProviderSettings />);
    await waitFor(() => expect(listCalls()).toEqual(["openrouter"]));

    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "openai" } });
    fireEvent.click(screen.getByRole("button", { name: "Refresh model list" }));
    await waitFor(() => expect(listCalls()).toEqual(["openrouter", "openai"]));
    await act(async () => resolveOpenRouter([{ id: "openai/gpt-4o", label: null }]));

    fireEvent.focus(screen.getByRole("combobox", { name: "Reasoning model" }));
    const options = screen.getAllByRole("option").map((option) => option.textContent);
    expect(options).toContain("gpt-4.1");
    expect(options).not.toContain("openai/gpt-4o");
  });

  it("offers no catalog refresh for a custom endpoint", async () => {
    saveAiProvider("custom");
    mockCatalog(true);
    render(<AiProviderSettings />);
    await act(async () => {});
    expect(screen.getByRole("textbox", { name: "Endpoint URL" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Refresh model list" })).not.toBeInTheDocument();
    expect(listCalls()).toEqual([]);
  });

  it("saves a reasoning model and an optional fast model separately", () => {
    render(<AiProviderSettings />);
    const fast = screen.getByRole("combobox", { name: "Fast model" });
    expect(fast).toHaveValue("");
    expect(fast).toHaveAttribute("placeholder", "Same as reasoning model");

    const reasoning = screen.getByRole("combobox", { name: "Reasoning model" });
    fireEvent.change(reasoning, { target: { value: "example/reasoning" } });
    fireEvent.blur(reasoning);
    fireEvent.change(fast, { target: { value: "example/fast" } });
    fireEvent.blur(fast);

    expect(readAiModel()).toBe("example/reasoning");
    expect(readAiFastModel()).toBe("example/fast");
    expect(screen.getByText(/Used for summaries, reply drafts, and contact enrichment/)).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Prices for example/fast" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Prices for example/reasoning" })).toBeInTheDocument();
  });

  it("groups the page by where AI shows and keeps models under a disclosure", () => {
    render(<AiProviderSettings />);
    for (const name of ["Connection", "Show AI In", "Automatic Briefs"]) {
      expect(screen.getByRole("heading", { name })).toBeInTheDocument();
    }
    expect(screen.queryByRole("heading", { name: "Features" })).not.toBeInTheDocument();
    expect(screen.getByText("Models and Usage").closest("details")).not.toHaveAttribute("open");
    expect(screen.getByRole("checkbox", { name: "Contacts" }))
      .toHaveAccessibleDescription();
    expect(screen.getByRole("checkbox", { name: "Writing" }))
      .toHaveAccessibleDescription();
    // No key is saved, so there is nothing to remove yet.
    expect(screen.getByRole("status")).toHaveTextContent("API key required");
    expect(screen.queryByRole("button", { name: "Remove Key" })).not.toBeInTheDocument();
  });

  it("opens the models disclosure for a custom endpoint without a model, since it has no default", () => {
    render(<AiProviderSettings />);
    const advanced = screen.getByText("Models and Usage").closest("details");
    expect(advanced).not.toHaveAttribute("open");
    fireEvent.change(screen.getByRole("combobox", { name: "Provider" }), { target: { value: "custom" } });
    expect(advanced).toHaveAttribute("open");
  });

  it("offers every on-demand area once a provider is chosen, and keeps automatic briefs off", () => {
    render(<AiProviderSettings />);
    for (const name of ["Reading", "Writing", "Contacts"]) {
      expect(screen.getByRole("checkbox", { name })).toBeChecked();
    }
    expect(screen.getByRole("radio", { name: "Never" })).toBeChecked();
    expect(readAiFeatures()).toMatchObject({ proactiveBriefs: false, proactiveKnownSendersOnly: false });
  });

  it("turns the whole Reading area off together and explains how to enter and leave chat", () => {
    render(<AiProviderSettings />);
    expect(screen.getByText(/press q or ⌘J to ask, and Escape returns to shortcuts/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "Reading" }));
    expect(readAiFeatures()).toMatchObject({ summarize: false, actionExtraction: false, threadChat: false });
  });

  it("saves automatic briefs as one choice and needs Reading for it", () => {
    render(<AiProviderSettings />);
    const group = screen.getByRole("radiogroup", { name: "Automatic briefs" });
    expect(group).toHaveAccessibleDescription();

    fireEvent.click(screen.getByRole("radio", { name: "People I’ve Emailed" }));
    expect(readAiFeatures()).toMatchObject({ proactiveBriefs: true, proactiveKnownSendersOnly: true });
    fireEvent.click(screen.getByRole("radio", { name: "All Conversations" }));
    expect(readAiFeatures()).toMatchObject({ proactiveBriefs: true, proactiveKnownSendersOnly: false });

    // Without Reading there is no brief to prepare, so the choice resets to Never.
    fireEvent.click(screen.getByRole("checkbox", { name: "Reading" }));
    expect(screen.getByRole("radio", { name: "Never" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "All Conversations" })).toBeDisabled();
    expect(group).toHaveAccessibleDescription(/Turn on Reading to use automatic briefs/);
    expect(readAiFeatures()).toMatchObject({ proactiveBriefs: false, proactiveKnownSendersOnly: false });
    expect(screen.getByRole("region", { name: "AI usage" })).toBeInTheDocument();
  });
});

