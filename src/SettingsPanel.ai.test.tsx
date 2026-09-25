import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { readAiFeatures, saveAiProvider } from "./aiSettings";
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
});
