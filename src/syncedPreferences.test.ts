import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { pullSyncedPreferences, queuePortablePreferences, queuePortablePreferencesAndWait } from "./syncedPreferences";
import { DEFAULT_AI_FEATURES, readAiFeatures, saveAiFeatures } from "./aiSettings";
import { readCalendarColors, resetCalendarColorsForTests, setCalendarColor } from "./calendarColors";
import { readWritingProfiles, resetWritingProfilesForTests, setWritingProfile } from "./writingProfiles";
import { readAvailabilityPreferences, readAvailabilityPreferencesFor, readAvailabilityProfiles, saveAvailabilityProfiles } from "./settings";
import type { AvailabilityProfiles } from "./domain";

const churchProfiles: AvailabilityProfiles = {
  defaultProfile: { id: "default", name: "Work" },
  otherProfiles: [{ id: "church", name: "Church", workingWindows: [{ weekday: 2, start: "18:30", end: "21:00" }], defaultDurationMinutes: 60, driveTimeBufferMinutes: 15 }],
  accountDefaults: { "home@example.com": "church" },
};

function queuedPreferences(): Record<string, unknown> {
  const call = vi.mocked(invoke).mock.calls.find(([command]) => command === "update_synced_preferences");
  expect(call).toBeDefined();
  return (call![1] as { preferences: Record<string, unknown> }).preferences;
}

describe("synced preferences data boundary", () => {
  beforeEach(() => {
    localStorage.clear();
    resetCalendarColorsForTests();
    resetWritingProfilesForTests();
    vi.clearAllMocks();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  });

  it("keeps appearance and selected navigation state off the replica", () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "private@example.com");
    localStorage.setItem("threestrands.theme", "dark");
    localStorage.setItem("threestrands.accent", "green");
    localStorage.setItem("threestrands.fontScale", "120");
    localStorage.setItem("threestrands.settings.fontFamily", "Georgia");
    localStorage.setItem("threestrands.settings.emailMinimumFontSize", "16");
    localStorage.setItem("threestrands.settings.emailZoom", "125");
    localStorage.setItem("threestrands.settings.emailTextColors", "app");

    queuePortablePreferences();

    const preferences = queuedPreferences();
    expect(preferences).not.toHaveProperty("selectedAccountId");
    expect(preferences).not.toHaveProperty("theme");
    expect(preferences).not.toHaveProperty("accent");
    expect(preferences).not.toHaveProperty("fontScale");
    expect(preferences).not.toHaveProperty("fontFamily");
    expect(preferences).not.toHaveProperty("emailMinimumFontSize");
    expect(preferences).not.toHaveProperty("emailZoom");
    expect(preferences).not.toHaveProperty("emailTextColors");
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

    const preferences = queuedPreferences();
    expect(Object.keys(preferences).sort()).toEqual([
      "aiEndpoint",
      "aiFastModel",
      "aiFeatures",
      "aiModel",
      "aiProvider",
      "autoReadDelaySeconds",
      "availabilityPreferences",
      "availabilityProfiles",
      "calendarColors",
      "loadRemoteImages",
      "writingProfiles",
    ]);
    const serialized = JSON.stringify(preferences);
    expect(serialized).not.toContain("secret");
    expect(serialized).not.toContain("private diagnostics");
  });

  it("applies synced preferences without replacing local appearance, fonts, or navigation", async () => {
    localStorage.setItem("threestrands.settings.selectedAccountId", "local@example.com");
    localStorage.setItem("threestrands.theme", "dark");
    localStorage.setItem("threestrands.accent", "teal");
    localStorage.setItem("threestrands.fontScale", "90");
    localStorage.setItem("threestrands.settings.fontFamily", "Helvetica");
    localStorage.setItem("threestrands.settings.emailMinimumFontSize", "12");
    localStorage.setItem("threestrands.settings.emailZoom", "125");
    localStorage.setItem("threestrands.settings.emailTextColors", "sender");
    vi.mocked(invoke).mockResolvedValue({
      // Older app versions replicated appearance fields.
      theme: "light",
      accent: "rose",
      fontScale: 110,
      emailMinimumFontSize: 18,
      emailZoom: 150,
      emailTextColors: "app",
      fontFamily: "Georgia",
      selectedAccountId: "remote@example.com",
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
        driveTimeBufferMinutes: 0,
      },
    });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(invoke).toHaveBeenCalledWith("synced_preferences");
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("local@example.com");
    expect(localStorage.getItem("threestrands.theme")).toBe("dark");
    expect(localStorage.getItem("threestrands.accent")).toBe("teal");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.accent).toBe("teal");
    expect(localStorage.getItem("threestrands.fontScale")).toBe("90");
    expect(localStorage.getItem("threestrands.settings.fontFamily")).toBe("Helvetica");
    expect(localStorage.getItem("threestrands.settings.emailMinimumFontSize")).toBe("12");
    expect(localStorage.getItem("threestrands.settings.emailZoom")).toBe("125");
    expect(localStorage.getItem("threestrands.settings.emailTextColors")).toBe("sender");
    expect(localStorage.getItem("threestrands.settings.autoReadDelaySeconds")).toBe("4");
    expect(localStorage.getItem("threestrands.crash-reports")).toBeNull();
  });

  it("queues calendar colors so they reach the user's other devices", () => {
    setCalendarColor("me@example.com", "primary", "teal");
    queuePortablePreferences();
    expect(queuedPreferences().calendarColors).toEqual({ "me@example.com": { primary: "teal" } });
  });

  it("applies calendar colors and resets chosen on another device", async () => {
    setCalendarColor("me@example.com", "primary", "teal");
    setCalendarColor("me@example.com", "team", "red");
    vi.mocked(invoke).mockResolvedValue({ calendarColors: { "me@example.com": { primary: "pink" } } });

    await expect(pullSyncedPreferences()).resolves.toBe(true);

    expect(readCalendarColors()).toEqual({ "me@example.com": { primary: "pink" } });
  });

  it("keeps this device's calendar colors when an older replica omits them", async () => {
    setCalendarColor("me@example.com", "primary", "teal");
    vi.mocked(invoke).mockResolvedValue({ fontScale: 110 });

    await pullSyncedPreferences();

    expect(readCalendarColors()).toEqual({ "me@example.com": { primary: "teal" } });
  });

  it("drops synced calendar colors outside the palette", async () => {
    vi.mocked(invoke).mockResolvedValue({
      calendarColors: { "me@example.com": { primary: "url(https://tracker.example/x)", team: "sky" } },
    });

    await pullSyncedPreferences();

    expect(readCalendarColors()).toEqual({ "me@example.com": { team: "sky" } });
  });

  it("queues writing profiles so Draft Review knows the user on their other devices", () => {
    setWritingProfile("Me@Example.com", { aboutYou: "CEO", reviewInstructions: "Be direct." });
    queuePortablePreferences();
    expect(queuedPreferences().writingProfiles).toEqual({ "me@example.com": { aboutYou: "CEO", reviewInstructions: "Be direct." } });
  });

  it("applies writing profiles edited on another device and keeps them when an older replica omits them", async () => {
    setWritingProfile("me@example.com", { aboutYou: "CEO", reviewInstructions: "" });
    setWritingProfile("home@example.com", { aboutYou: "Parent", reviewInstructions: "" });
    vi.mocked(invoke).mockResolvedValue({ writingProfiles: { "me@example.com": { aboutYou: "Founder", reviewInstructions: "Push for a call." } } });

    await pullSyncedPreferences();
    expect(readWritingProfiles()).toEqual({ "me@example.com": { aboutYou: "Founder", reviewInstructions: "Push for a call." } });

    vi.mocked(invoke).mockResolvedValue({ autoReadDelaySeconds: 4 });
    await pullSyncedPreferences();
    expect(readWritingProfiles()).toEqual({ "me@example.com": { aboutYou: "Founder", reviewInstructions: "Push for a call." } });
  });

  it("queues availability profiles with the default profile's hours for devices without profiles", () => {
    saveAvailabilityProfiles(churchProfiles);
    queuePortablePreferences();
    expect(queuedPreferences().availabilityProfiles).toEqual(churchProfiles);
    expect(queuedPreferences().availabilityPreferences).toEqual(readAvailabilityPreferences());
  });

  it("applies profiles from another device and keeps them when an older replica omits them", async () => {
    vi.mocked(invoke).mockResolvedValue({ availabilityProfiles: churchProfiles });
    await pullSyncedPreferences();
    expect(readAvailabilityProfiles()).toEqual(churchProfiles);

    vi.mocked(invoke).mockResolvedValue({ autoReadDelaySeconds: 4 });
    await pullSyncedPreferences();
    expect(readAvailabilityProfiles()).toEqual(churchProfiles);
  });

  it("applies hours an older device changed to the default profile and leaves the other profiles alone", async () => {
    saveAvailabilityProfiles(churchProfiles);
    const older = { ...readAvailabilityPreferences(), workingWindows: [{ weekday: 1, start: "07:00", end: "15:00" }] };
    // An older device knows only one set of hours and sends no profiles.
    vi.mocked(invoke).mockResolvedValue({ availabilityPreferences: older });

    await pullSyncedPreferences();

    expect(readAvailabilityPreferencesFor("work@example.com")).toEqual(older);
    expect(readAvailabilityProfiles()).toEqual(churchProfiles);
    expect(readAvailabilityPreferencesFor("home@example.com").workingWindows).toEqual(churchProfiles.otherProfiles[0].workingWindows);
  });

  it("preserves the device appearance when a new replica omits it", async () => {
    localStorage.setItem("threestrands.theme", "light");
    localStorage.setItem("threestrands.accent", "graphite");
    localStorage.setItem("threestrands.fontScale", "90");
    vi.mocked(invoke).mockResolvedValue({ autoReadDelaySeconds: 4 });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(localStorage.getItem("threestrands.theme")).toBe("light");
    expect(localStorage.getItem("threestrands.accent")).toBe("graphite");
    expect(localStorage.getItem("threestrands.fontScale")).toBe("90");
    expect(localStorage.getItem("threestrands.settings.autoReadDelaySeconds")).toBe("4");
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
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, summarize: true, proactiveBriefs: true, proactiveKnownSendersOnly: true, threadChat: true });
    vi.mocked(invoke).mockResolvedValue({
      aiFeatures: { draftAssist: true, summarize: true, actionExtraction: true, contactEnrichment: false },
    });

    expect(await pullSyncedPreferences()).toBe(true);
    expect(readAiFeatures()).toMatchObject({ draftAssist: true, actionExtraction: true, proactiveBriefs: true, proactiveKnownSendersOnly: true, threadChat: true });
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
    // Contact Enrichment starts on, so switching it off is the local change.
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, contactEnrichment: false });
    resolvePreferences({
      autoReadDelaySeconds: 4,
      aiFeatures: { ...DEFAULT_AI_FEATURES, contactEnrichment: true },
    });

    expect(await pulling).toBe(true);
    expect(readAiFeatures().contactEnrichment).toBe(false);
    expect(localStorage.getItem("threestrands.settings.autoReadDelaySeconds")).toBe("4");
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
