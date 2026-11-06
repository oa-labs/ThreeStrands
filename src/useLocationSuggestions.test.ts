import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { LOCATION_SEARCH_DELAY_MS, useLocationSuggestions } from "./useLocationSuggestions";

describe("useLocationSuggestions", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });

  const settle = async (ms = LOCATION_SEARCH_DELAY_MS) => { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); };

  it("searches once after a pause in typing, with the trimmed text", async () => {
    const search = vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(["Alloy 26, 100 S Commons"]);
    const { result, rerender } = renderHook(({ text }) => useLocationSuggestions(text, true), { initialProps: { text: "all" } });
    await settle(LOCATION_SEARCH_DELAY_MS - 1);
    rerender({ text: "allo" });
    rerender({ text: " alloy " });
    await settle(LOCATION_SEARCH_DELAY_MS - 1);
    expect(search).not.toHaveBeenCalled();
    await settle(1);
    expect(search).toHaveBeenCalledTimes(1);
    expect(search).toHaveBeenCalledWith("alloy");
    expect(result.current).toEqual(["Alloy 26, 100 S Commons"]);
  });

  it.each([
    ["two characters", "al", true],
    ["two characters padded with spaces", "  al  ", true],
    ["a disabled field", "alloy", false],
  ])("does not search for %s", async (_name, text, enabled) => {
    const search = vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(["Alloy 26"]);
    const { result } = renderHook(() => useLocationSuggestions(text, enabled));
    await settle();
    expect(search).not.toHaveBeenCalled();
    expect(result.current).toEqual([]);
  });

  it("counts characters, not bytes, toward the minimum", async () => {
    const search = vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue([]);
    renderHook(() => useLocationSuggestions("日本語", true));
    await settle();
    expect(search).toHaveBeenCalledWith("日本語");
  });

  it("keeps only the latest text's answer when an earlier search finishes last", async () => {
    let finishEarlier!: (value: string[]) => void;
    vi.spyOn(mailClient, "searchEventLocations").mockImplementation((query) => query === "all"
      ? new Promise((resolve) => { finishEarlier = resolve; })
      : Promise.resolve(["Alloy 26"]));
    const { result, rerender } = renderHook(({ text }) => useLocationSuggestions(text, true), { initialProps: { text: "all" } });
    await settle();
    rerender({ text: "alloy" });
    await settle();
    expect(result.current).toEqual(["Alloy 26"]);
    await act(async () => { finishEarlier(["Allegheny Center"]); });
    expect(result.current).toEqual(["Alloy 26"]);
  });

  it("clears suggestions when the text becomes too short or the field is disabled", async () => {
    vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(["Alloy 26"]);
    const { result, rerender } = renderHook(({ text, enabled }) => useLocationSuggestions(text, enabled), { initialProps: { text: "alloy", enabled: true } });
    await settle();
    expect(result.current).toEqual(["Alloy 26"]);
    rerender({ text: "al", enabled: true });
    expect(result.current).toEqual([]);
    rerender({ text: "alloy", enabled: false });
    expect(result.current).toEqual([]);
  });

  it("offers nothing when the search fails", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(mailClient, "searchEventLocations").mockRejectedValue(new Error("Offline"));
    const { result } = renderHook(() => useLocationSuggestions("alloy", true));
    await settle();
    expect(result.current).toEqual([]);
  });
});
