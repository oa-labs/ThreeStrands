import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_AI_FEATURES, isAiApiKeyConfigured, readAiFeatures, readAiProvider } from "./aiSettings";
import { useAiAvailability } from "./useAiAvailability";

vi.mock("./aiSettings", async (original) => ({ ...await original<typeof import("./aiSettings")>(), isAiApiKeyConfigured: vi.fn(), readAiProvider: vi.fn(), readAiFeatures: vi.fn() }));
beforeEach(() => {
  vi.mocked(readAiProvider).mockReturnValue("openai");
  vi.mocked(readAiFeatures).mockReturnValue({ ...DEFAULT_AI_FEATURES, draftAssist: true, contactEnrichment: true });
  vi.mocked(isAiApiKeyConfigured).mockResolvedValue(true);
});
afterEach(() => vi.resetAllMocks());

describe("useAiAvailability", () => {
  it("shares draft and contact feature readiness", async () => {
    const { result } = renderHook(() => useAiAvailability(false));
    await waitFor(() => expect(result.current.aiDraftAvailable).toBe(true));
    expect(result.current.aiContactAvailable).toBe(true);
    expect(result.current.aiContactFeatureEnabled).toBe(true);
  });

  it.each([false, "error"])("keeps setup actions enabled when the key check returns %s", async (status) => {
    if (status === "error") vi.mocked(isAiApiKeyConfigured).mockRejectedValue(Error("offline"));
    else vi.mocked(isAiApiKeyConfigured).mockResolvedValue(false);
    const { result } = renderHook(() => useAiAvailability(false));
    await act(async () => {});
    expect(result.current.aiDraftFeatureEnabled).toBe(true);
    expect(result.current.aiContactFeatureEnabled).toBe(true);
    expect(result.current.aiDraftAvailable).toBe(false);
    expect(result.current.aiContactAvailable).toBe(false);
  });

  it("ignores an older key check after settings disable the provider", async () => {
    let resolve!: (configured: boolean) => void;
    vi.mocked(isAiApiKeyConfigured).mockReturnValue(new Promise<boolean>((done) => { resolve = done; }));
    const { result, rerender } = renderHook(({ open }) => useAiAvailability(open), { initialProps: { open: false } });
    rerender({ open: true });
    vi.mocked(readAiProvider).mockReturnValue("none");
    rerender({ open: false });
    await act(async () => resolve(true));
    expect(result.current.aiDraftFeatureEnabled).toBe(false);
    expect(result.current.aiContactFeatureEnabled).toBe(false);
    expect(result.current.aiDraftAvailable).toBe(false);
    expect(result.current.aiContactAvailable).toBe(false);
  });

  it("checks the key when only Contact Enrichment is enabled", async () => {
    vi.mocked(readAiFeatures).mockReturnValue({ ...DEFAULT_AI_FEATURES, summarize: false, actionExtraction: false, threadChat: false, draftAssist: false, contactEnrichment: true });
    const { result } = renderHook(() => useAiAvailability(false));
    await waitFor(() => expect(result.current.aiContactAvailable).toBe(true));
    expect(result.current.aiDraftAvailable).toBe(false);
  });
});
