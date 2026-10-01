import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { readAiFastModel, readAiFeatures, readAiModel, saveAiProvider } from "./aiSettings";
import { AiProviderSettings } from "./SettingsPanel";
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

    act(() => {
      fireEvent.click(screen.getByRole("checkbox", { name: "Draft Assist" }));
      fireEvent.click(screen.getByRole("checkbox", { name: "Contact Enrichment" }));
    });

    await waitFor(() => expect(vi.mocked(invoke).mock.calls
      .filter(([command]) => command === "update_synced_preferences")).toHaveLength(2));
    const queued = vi.mocked(invoke).mock.calls
      .filter(([command]) => command === "update_synced_preferences");
    expect(queued).toHaveLength(2);
    expect(queued[1][1]).toEqual({
      preferences: expect.objectContaining({
        aiFeatures: expect.objectContaining({ draftAssist: true, contactEnrichment: true }),
      }),
    });
    expect(readAiFeatures()).toMatchObject({ draftAssist: true, contactEnrichment: true });
  });

  it("saves a reasoning model and an optional fast model separately", () => {
    render(<AiProviderSettings />);
    const fast = screen.getByRole("textbox", { name: "Fast model" });
    expect(fast).toHaveValue("");
    expect(fast).toHaveAttribute("placeholder", "Same as reasoning model");

    fireEvent.change(screen.getByRole("textbox", { name: "Reasoning model" }), { target: { value: "example/reasoning" } });
    fireEvent.change(fast, { target: { value: "example/fast" } });

    expect(readAiModel()).toBe("example/reasoning");
    expect(readAiFastModel()).toBe("example/fast");
    expect(screen.getByText(/Used for summaries, reply drafts, and contact enrichment/)).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Prices for example/fast" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Prices for example/reasoning" })).toBeInTheDocument();
  });

  it("saves the Thread Chat switch and explains how to enter and leave it", () => {
    render(<AiProviderSettings />);
    fireEvent.click(screen.getByRole("checkbox", { name: "Thread Chat" }));
    expect(readAiFeatures().threadChat).toBe(true);
    expect(screen.getByText(/Press q or ⌘J to ask; Escape returns to shortcuts/)).toBeInTheDocument();
  });

  it("offers proactive suggestions only once a brief feature is on, and the sender filter only once proactive is on", () => {
    render(<AiProviderSettings />);
    const proactive = screen.getByRole("checkbox", { name: "Proactive Suggestions" });
    const knownOnly = screen.getByRole("checkbox", { name: "Only for People I’ve Emailed" });
    expect(proactive).toBeDisabled();
    expect(knownOnly).toBeDisabled();

    fireEvent.click(screen.getByRole("checkbox", { name: "Suggestions" }));
    expect(proactive).toBeEnabled();
    expect(knownOnly).toBeDisabled();

    fireEvent.click(proactive);
    expect(knownOnly).toBeEnabled();
    fireEvent.click(knownOnly);
    expect(readAiFeatures()).toMatchObject({ actionExtraction: true, proactiveBriefs: true, proactiveKnownSendersOnly: true });
    expect(screen.getByText(/at least 3 seconds/)).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "AI usage" })).toBeInTheDocument();
  });
});
