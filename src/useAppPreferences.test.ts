import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { useAppPreferences } from "./useAppPreferences";

afterEach(() => {
  localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.style.removeProperty("--font-scale");
});

describe("useAppPreferences", () => {
  it("persists and applies preference updates", () => {
    const { result } = renderHook(() => useAppPreferences());

    act(() => result.current.setTheme("dark"));
    act(() => result.current.setFontScale(140));
    act(() => result.current.setFontFamily("system"));
    act(() => result.current.setAutoReadDelaySeconds(10));
    act(() => result.current.setLoadRemoteImages(true));

    expect(result.current.theme).toBe("dark");
    expect(result.current.fontScale).toBe(140);
    expect(result.current.fontFamily).toBe("system");
    expect(result.current.autoReadDelaySeconds).toBe(10);
    expect(result.current.loadRemoteImages).toBe(true);
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.style.getPropertyValue("--font-scale")).toBe("1.4");
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
