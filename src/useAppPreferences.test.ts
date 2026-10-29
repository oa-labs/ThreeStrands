import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { readAvailabilityPreferences, saveAvailabilityPreferences, SYSTEM_FONT_STACK } from "./settings";
import { useAppPreferences } from "./useAppPreferences";

const { listenMock, pullMock } = vi.hoisted(() => ({
  listenMock: vi.fn(async () => () => {}),
  pullMock: vi.fn(async () => true),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
vi.mock("./syncedPreferences", () => ({ pullSyncedPreferences: pullMock, queuePortablePreferences: vi.fn() }));

afterEach(() => {
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  listenMock.mockClear();
  pullMock.mockClear();
  localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.removeAttribute("data-accent");
  document.documentElement.style.removeProperty("--font-scale");
  document.documentElement.style.removeProperty("--font-family");
});

describe("useAppPreferences", () => {
  it("persists and applies preference updates", () => {
    const { result } = renderHook(() => useAppPreferences());
    expect(result.current.fontFamily).toBe("system");

    act(() => result.current.setTheme("dark"));
    act(() => result.current.setAccent("green"));
    act(() => result.current.setFontScale(140));
    act(() => result.current.setFontFamily("Georgia"));
    act(() => result.current.setEmailMinimumFontSize(18));
    act(() => result.current.setEmailZoom(125));
    act(() => result.current.setAutoReadDelaySeconds(10));
    act(() => result.current.setLoadRemoteImages(true));

    expect(result.current.theme).toBe("dark");
    expect(result.current.accent).toBe("green");
    expect(result.current.fontScale).toBe(140);
    expect(result.current.fontFamily).toBe("Georgia");
    expect(result.current.emailMinimumFontSize).toBe(18);
    expect(result.current.emailZoom).toBe(125);
    expect(localStorage.getItem("threestrands.settings.emailZoom")).toBe("125");
    expect(localStorage.getItem("threestrands.settings.emailMinimumFontSize")).toBe("18");
    expect(result.current.autoReadDelaySeconds).toBe(10);
    expect(result.current.loadRemoteImages).toBe(true);
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.accent).toBe("green");
    expect(document.documentElement.style.getPropertyValue("--font-scale")).toBe("1.4");
    expect(document.documentElement.style.getPropertyValue("--font-family")).toBe(`"Georgia", ${SYSTEM_FONT_STACK}`);
    expect(localStorage.getItem("threestrands.accent")).toBe("green");
    expect(localStorage.getItem("threestrands.settings.fontFamily")).toBe("Georgia");
  });

  it("keeps availability preferences stable across syncs that leave them unchanged", async () => {
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const { result } = renderHook(() => useAppPreferences());
    await waitFor(() => expect(listenMock).toHaveBeenCalled());
    const onSyncStatus = (listenMock.mock.calls[0] as unknown as [string, () => void])[1];
    await waitFor(() => expect(pullMock).toHaveBeenCalledTimes(1));
    const before = result.current.availabilityPreferences;

    // Another preference changed: meeting cards must not see a new object.
    await act(async () => { onSyncStatus(); await Promise.resolve(); });
    expect(pullMock).toHaveBeenCalledTimes(2);
    expect(result.current.availabilityPreferences).toBe(before);

    saveAvailabilityPreferences({ ...readAvailabilityPreferences(), defaultDurationMinutes: before.defaultDurationMinutes + 15 });
    await act(async () => { onSyncStatus(); await Promise.resolve(); });
    expect(result.current.availabilityPreferences).not.toBe(before);
    expect(result.current.availabilityPreferences.defaultDurationMinutes).toBe(before.defaultDurationMinutes + 15);
  });

  it("gives each account its profile's preferences, the same object until settings change", () => {
    const { result } = renderHook(() => useAppPreferences());
    const church = { id: "church", name: "Church", workingWindows: [{ weekday: 2, start: "18:30", end: "21:00" }], defaultDurationMinutes: 60, driveTimeBufferMinutes: 15 };
    act(() => result.current.setAvailabilityProfiles({ ...result.current.availabilityProfiles, otherProfiles: [church], accountDefaults: { "home@example.com": "church" } }));

    const home = result.current.availabilityFor("Home@example.com");
    const shared = result.current.availabilityPreferences;
    expect(home).toEqual({
      timeZone: shared.timeZone,
      slotIncrementMinutes: shared.slotIncrementMinutes,
      workingWindows: church.workingWindows,
      defaultDurationMinutes: 60,
      driveTimeBufferMinutes: 15,
    });
    expect(result.current.availabilityFor("work@example.com")).toBe(result.current.availabilityPreferences);
    expect(result.current.availabilityFor(null)).toBe(result.current.availabilityPreferences);

    // An unrelated preference change keeps the object, so meeting cards don't re-check the calendar.
    act(() => result.current.setAutoReadDelaySeconds(5));
    expect(result.current.availabilityFor("home@example.com")).toBe(home);

    // The shared timezone reaches every profile.
    act(() => result.current.setAvailabilityPreferences({ ...result.current.availabilityPreferences, timeZone: "Asia/Tokyo" }));
    expect(result.current.availabilityFor("home@example.com").timeZone).toBe("Asia/Tokyo");
  });

  it("adjusts font scale through the shared bounds", () => {
    const { result } = renderHook(() => useAppPreferences());

    act(() => result.current.setFontScale(140));
    act(() => result.current.adjustFontScale(1));
    expect(result.current.fontScale).toBe(140);
    act(() => result.current.adjustFontScale(-1));
    expect(result.current.fontScale).toBe(130);
  });
});
