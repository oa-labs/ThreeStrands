import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { pullSyncedPreferences, queuePortablePreferences, queuePortablePreferencesAndWait } from "./syncedPreferences";
import { DEFAULT_AI_FEATURES, readAiFeatures, saveAiFeatures } from "./aiSettings";

describe("synced preferences data boundary", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });

  it("keeps appearance and selected navigation state off the replica", () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "private@example.com");
    localStorage.setItem("threestrands.theme", "dark");
    localStorage.setItem("threestrands.accent", "green");

    queuePortablePreferences();

    expect(invoke).toHaveBeenCalledWith("update_synced_preferences", {
      preferences: expect.not.objectContaining({
        selectedAccountId: expect.anything(),
        theme: expect.anything(),
        accent: expect.anything(),
      }),
    });
  });

  it("does not queue or pull preferences outside the native app", async () => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");

    queuePortablePreferences();

    expect(await pullSyncedPreferences()).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("queues only the typed portable preference allowlist", () => {
    localStorage.setItem("threestrands.crash-reports", "private diagnostics");
    localStorage.setItem("threestrands.settings.ai.apiKey", "secret");

    queuePortablePreferences();

    expect(invoke).toHaveBeenCalledWith("update_synced_preferences", {
      preferences: expect.not.objectContaining({
        selectedAccountId: expect.anything(),
        crashReports: expect.anything(),
        apiKey: expect.anything(),
      }),
    });
  });

  it("applies synced preferences without replacing local appearance or navigation", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "local@example.com");
    localStorage.setItem("threestrands.theme", "dark");
    localStorage.setItem("threestrands.accent", "teal");
    vi.mocked(invoke).mockResolvedValue({
      theme: "light", // Legacy replica field from an older app version.
      accent: "rose", // Legacy replica field from an older app version.
      selectedAccountId: "remote@example.com",
      fontScale: 110,
      fontFamily: "Georgia",
      autoReadDelaySeconds: 4,
      loadRemoteImages: false,
      aiProvider: "none",
      aiModel: "",
      aiEndpoint: "",
      aiFeatures: { draftAssist: false, summarize: false, actionExtraction: false, contactEnrichment: false },
      availabilityPreferences: {
        timeZone: "UTC",
        workingWindows: [],
        defaultDurationMinutes: 30,
        slotIncrementMinutes: 15,
      },
    });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(invoke).toHaveBeenCalledWith("synced_preferences");
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("local@example.com");
    expect(localStorage.getItem("threestrands.theme")).toBe("dark");
    expect(localStorage.getItem("threestrands.accent")).toBe("teal");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.accent).toBe("teal");
    expect(localStorage.getItem("threestrands.fontScale")).toBe("110");
    expect(localStorage.getItem("threestrands.crash-reports")).toBeNull();
  });

  it("preserves the device theme when a new replica omits it", async () => {
    localStorage.setItem("threestrands.theme", "light");
    localStorage.setItem("threestrands.accent", "graphite");
    vi.mocked(invoke).mockResolvedValue({ fontScale: 120 });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(localStorage.getItem("threestrands.theme")).toBe("light");
    expect(localStorage.getItem("threestrands.accent")).toBe("graphite");
    expect(localStorage.getItem("threestrands.fontScale")).toBe("120");
  });

  it("keeps contact enrichment on when an older replica omits that feature", async () => {
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, contactEnrichment: true });
    vi.mocked(invoke).mockResolvedValue({
      aiFeatures: { draftAssist: false, summarize: true, actionExtraction: false },
    });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(readAiFeatures()).toMatchObject({ summarize: true, contactEnrichment: true });
  });

  it("keeps proactive suggestions as set on this device when an older replica omits them", async () => {
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, summarize: true, proactiveBriefs: true, proactiveKnownSendersOnly: true });
    vi.mocked(invoke).mockResolvedValue({
      aiFeatures: { draftAssist: true, summarize: true, actionExtraction: true, contactEnrichment: false },
    });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(readAiFeatures()).toMatchObject({ draftAssist: true, actionExtraction: true, proactiveBriefs: true, proactiveKnownSendersOnly: true });
  });

  it("applies proactive suggestions turned off on another device", async () => {
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, proactiveBriefs: true });
    vi.mocked(invoke).mockResolvedValue({ aiFeatures: { ...DEFAULT_AI_FEATURES, proactiveBriefs: false } });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(readAiFeatures().proactiveBriefs).toBe(false);
  });

  it("preserves a feature setting changed while a synced preference read is in flight", async () => {
    let resolvePreferences!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(() => new Promise((resolve) => { resolvePreferences = resolve; }));

    const pulling = pullSyncedPreferences();
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, contactEnrichment: true });
    resolvePreferences({
      fontScale: 120,
      aiFeatures: { ...DEFAULT_AI_FEATURES, contactEnrichment: false },
    });

    expect(await pulling).toBe(true);
    expect(readAiFeatures().contactEnrichment).toBe(true);
    expect(localStorage.getItem("threestrands.fontScale")).toBe("120");
  });

  it("waits for a queued feature change before reading synced preferences", async () => {
    let resolveWrite!: () => void;
    let stored: unknown = { aiFeatures: { ...DEFAULT_AI_FEATURES } };
    vi.mocked(invoke).mockImplementation((command, args) => {
      if (command === "update_synced_preferences") {
        return new Promise<void>((resolve) => {
          resolveWrite = () => { stored = (args as { preferences: unknown }).preferences; resolve(); };
        });
      }
      if (command === "synced_preferences") return Promise.resolve(stored);
      return Promise.resolve(undefined);
    });

    saveAiFeatures({ ...DEFAULT_AI_FEATURES, contactEnrichment: true });
    const queued = queuePortablePreferencesAndWait();
    const pulling = pullSyncedPreferences();
    expect(invoke).not.toHaveBeenCalledWith("synced_preferences");

    resolveWrite();
    await queued;
    expect(await pulling).toBe(true);
    expect(readAiFeatures().contactEnrichment).toBe(true);
    expect(invoke).toHaveBeenCalledWith("synced_preferences");
  });

  it("writes rapid feature changes in the order they were queued", async () => {
    let resolveFirst!: () => void;
    let writeCount = 0;
    vi.mocked(invoke).mockImplementation((command) => {
      if (command !== "update_synced_preferences") return Promise.resolve(null);
      writeCount++;
      if (writeCount === 1) {
        return new Promise<void>((resolve) => { resolveFirst = resolve; });
      }
      return Promise.resolve();
    });

    saveAiFeatures({ ...DEFAULT_AI_FEATURES, contactEnrichment: true });
    const first = queuePortablePreferencesAndWait();
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, contactEnrichment: false });
    const second = queuePortablePreferencesAndWait();
    expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "update_synced_preferences")).toHaveLength(1);

    resolveFirst();
    await Promise.all([first, second]);
    const writes = vi.mocked(invoke).mock.calls.filter(([name]) => name === "update_synced_preferences");
    expect(writes).toHaveLength(2);
    expect((writes[0][1] as { preferences: { aiFeatures: { contactEnrichment: boolean } } }).preferences.aiFeatures.contactEnrichment).toBe(true);
    expect((writes[1][1] as { preferences: { aiFeatures: { contactEnrichment: boolean } } }).preferences.aiFeatures.contactEnrichment).toBe(false);
  });
});
