import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { SYSTEM_FONT_STACK } from "./settings";
import { useAppPreferences } from "./useAppPreferences";

afterEach(() => {
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
    act(() => result.current.setAutoReadDelaySeconds(10));
    act(() => result.current.setLoadRemoteImages(true));

    expect(result.current.theme).toBe("dark");
    expect(result.current.accent).toBe("green");
    expect(result.current.fontScale).toBe(140);
    expect(result.current.fontFamily).toBe("Georgia");
    expect(result.current.autoReadDelaySeconds).toBe(10);
    expect(result.current.loadRemoteImages).toBe(true);
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.accent).toBe("green");
    expect(document.documentElement.style.getPropertyValue("--font-scale")).toBe("1.4");
    expect(document.documentElement.style.getPropertyValue("--font-family")).toBe(`"Georgia", ${SYSTEM_FONT_STACK}`);
    expect(localStorage.getItem("threestrands.accent")).toBe("green");
    expect(localStorage.getItem("threestrands.settings.fontFamily")).toBe("Georgia");
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
