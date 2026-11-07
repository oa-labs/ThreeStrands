import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { jsonEqual, useStableValue } from "./useStableValue";

describe("useStableValue", () => {
  it("keeps the first value while equal copies arrive, then takes a changed one", () => {
    const first = { ids: ["a", "b"] };
    const { result, rerender } = renderHook(({ value }) => useStableValue(value), { initialProps: { value: first } });
    expect(result.current).toBe(first);
    rerender({ value: { ids: ["a", "b"] } });
    expect(result.current).toBe(first);
    const changed = { ids: ["a"] };
    rerender({ value: changed });
    expect(result.current).toBe(changed);
    rerender({ value: { ids: ["a"] } });
    expect(result.current).toBe(changed);
  });

  it("uses the given comparison", () => {
    const byId = (left: { id: string; label: string }, right: { id: string; label: string }) => left.id === right.id;
    const first = { id: "1", label: "One" };
    const { result, rerender } = renderHook(({ value }) => useStableValue(value, byId), { initialProps: { value: first } });
    rerender({ value: { id: "1", label: "Renamed" } });
    expect(result.current).toBe(first);
    rerender({ value: { id: "2", label: "Two" } });
    expect(result.current).toEqual({ id: "2", label: "Two" });
  });

  it("handles missing values", () => {
    const initialProps: { value: { id: string } | undefined } = { value: undefined };
    const { result, rerender } = renderHook(({ value }) => useStableValue(value), { initialProps });
    expect(result.current).toBeUndefined();
    rerender({ value: { id: "1" } });
    expect(result.current).toEqual({ id: "1" });
    rerender({ value: undefined });
    expect(result.current).toBeUndefined();
  });

  it("compares plain data by content", () => {
    expect(jsonEqual({ a: [1, 2] }, { a: [1, 2] })).toBe(true);
    expect(jsonEqual({ a: [1, 2] }, { a: [2, 1] })).toBe(false);
  });
});
