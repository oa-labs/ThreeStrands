import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { readLocalStorage, useLocalStorageState } from "./useLocalStorageState";

afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); localStorage.clear(); });
const readNumber = (key: string) => readLocalStorage(key, (raw) => {
  if (raw === null || !Number.isFinite(Number(raw))) return 10;
  return Number(raw);
}, 10);

describe("useLocalStorageState", () => {
  it("restores existing formats and persists consecutive functional updates", () => {
    localStorage.setItem("width", "40");
    const { result, unmount } = renderHook(() => useLocalStorageState("width", () => readNumber("width")));
    expect(result.current[0]).toBe(40);
    const setter = result.current[1];
    act(() => { setter((n) => n + 1); setter((n) => n + 1); });
    expect(result.current[0]).toBe(42);
    expect(result.current[1]).toBe(setter);
    expect(localStorage.getItem("width")).toBe("42");
    unmount();
    expect(renderHook(() => useLocalStorageState("width", () => readNumber("width"))).result.current[0]).toBe(42);
  });

  it("retains session state with blocked reads and writes", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw Error("blocked"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw Error("quota"); });
    const { result } = renderHook(() => useLocalStorageState("width", () => readNumber("width")));
    expect(result.current[0]).toBe(10);
    act(() => result.current[1](80));
    expect(result.current[0]).toBe(80);
  });

  it("handles malformed JSON and preserves custom set serialization", () => {
    localStorage.setItem("collapsed", "{");
    const read = () => readLocalStorage("collapsed", (raw) => new Set<string>(JSON.parse(raw ?? "[]")), new Set<string>());
    const serialize = (value: Set<string>) => JSON.stringify([...value]);
    const { result } = renderHook(() => useLocalStorageState("collapsed", read, serialize));
    expect(result.current[0].size).toBe(0);
    act(() => result.current[1](new Set(["calendar"])));
    expect(localStorage.getItem("collapsed")).toBe('["calendar"]');
  });

  it("loads a changed key without copying the previous preference", () => {
    localStorage.setItem("a", "20"); localStorage.setItem("b", "30");
    const { result, rerender } = renderHook(({ key }) => useLocalStorageState(key, () => readNumber(key)), { initialProps: { key: "a" } });
    rerender({ key: "b" });
    expect(result.current[0]).toBe(30);
    expect(localStorage.getItem("a")).toBe("20");
    expect(localStorage.getItem("b")).toBe("30");
  });

  it("debounces persistence and cancels pending writes when unmounted", () => {
    vi.useFakeTimers();
    const { result, unmount } = renderHook(() => useLocalStorageState("width", () => 10, String, 150));
    act(() => result.current[1](20));
    act(() => vi.advanceTimersByTime(149));
    expect(localStorage.getItem("width")).toBeNull();
    act(() => vi.advanceTimersByTime(1));
    expect(localStorage.getItem("width")).toBe("20");
    act(() => result.current[1](30));
    unmount();
    act(() => vi.runAllTimers());
    expect(localStorage.getItem("width")).toBe("20");
  });
});
