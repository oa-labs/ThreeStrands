import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ReadingSettings } from "./ReadingSettings";
import { DEFAULT_AUTO_READ_DELAY_SECONDS } from "./settings";
import { useAppPreferences } from "./useAppPreferences";

/** Renders the pane against the real stored preferences, re-rendering on each change. */
function renderWithPreferences() {
  const hook = renderHook(() => useAppPreferences());
  const view = () => (
    <ReadingSettings
      markReadOnArchive={hook.result.current.markReadOnArchive}
      onMarkReadOnArchiveChange={hook.result.current.setMarkReadOnArchive}
      markReadOnOpen={hook.result.current.markReadOnOpen}
      onMarkReadOnOpenChange={hook.result.current.setMarkReadOnOpen}
      autoReadDelaySeconds={hook.result.current.autoReadDelaySeconds}
      onAutoReadDelayChange={hook.result.current.setAutoReadDelaySeconds}
    />
  );
  const rendered = render(view());
  const rerender = () => rendered.rerender(view());
  return { hook, rerender };
}

const openMode = () => screen.getByRole("combobox", { name: "Mark Read After Opening" });
const delay = () => screen.queryByRole("spinbutton", { name: "Mark Read Delay in Seconds" });

describe("Mark as Read settings", () => {
  beforeEach(() => localStorage.clear());

  it("starts on After a Delay with the default delay and describes both settings", () => {
    renderWithPreferences();
    expect(openMode()).toHaveValue("delay");
    expect(delay()).toHaveValue(DEFAULT_AUTO_READ_DELAY_SECONDS);
    expect(openMode()).toHaveAccessibleDescription();
    expect(delay()).toHaveAccessibleDescription();
    expect(screen.getByRole("checkbox", { name: "Mark Read When Archiving" })).toHaveAccessibleDescription();
  });

  it("stores Immediately as a zero delay and hides the delay field", () => {
    const { hook, rerender } = renderWithPreferences();
    act(() => { fireEvent.change(openMode(), { target: { value: "immediately" } }); });
    rerender();
    expect(hook.result.current.autoReadDelaySeconds).toBe(0);
    expect(hook.result.current.markReadOnOpen).toBe(true);
    expect(openMode()).toHaveValue("immediately");
    expect(delay()).not.toBeInTheDocument();
  });

  it("returns from Immediately to the default delay", () => {
    localStorage.setItem("threestrands.settings.autoReadDelaySeconds", "0");
    const { hook, rerender } = renderWithPreferences();
    expect(openMode()).toHaveValue("immediately");
    act(() => { fireEvent.change(openMode(), { target: { value: "delay" } }); });
    rerender();
    expect(hook.result.current.autoReadDelaySeconds).toBe(DEFAULT_AUTO_READ_DELAY_SECONDS);
    expect(delay()).toHaveValue(DEFAULT_AUTO_READ_DELAY_SECONDS);
  });

  it("keeps the chosen delay through Never and back", () => {
    localStorage.setItem("threestrands.settings.autoReadDelaySeconds", "15");
    const { hook, rerender } = renderWithPreferences();
    act(() => { fireEvent.change(openMode(), { target: { value: "never" } }); });
    rerender();
    expect(hook.result.current.markReadOnOpen).toBe(false);
    expect(localStorage.getItem("threestrands.settings.markReadOnOpen")).toBe("false");
    expect(openMode()).toHaveValue("never");
    expect(delay()).not.toBeInTheDocument();

    act(() => { fireEvent.change(openMode(), { target: { value: "delay" } }); });
    rerender();
    expect(hook.result.current.markReadOnOpen).toBe(true);
    expect(delay()).toHaveValue(15);
  });

  it("ignores a zero typed into the delay field instead of switching to Immediately", () => {
    const { hook, rerender } = renderWithPreferences();
    act(() => { fireEvent.change(delay()!, { target: { value: "0" } }); });
    rerender();
    expect(hook.result.current.autoReadDelaySeconds).toBe(DEFAULT_AUTO_READ_DELAY_SECONDS);
    act(() => { fireEvent.change(delay()!, { target: { value: "9" } }); });
    rerender();
    expect(hook.result.current.autoReadDelaySeconds).toBe(9);
  });

  it("describes the effect of each choice differently", () => {
    const { rerender } = renderWithPreferences();
    const descriptions = new Set<string>();
    for (const mode of ["immediately", "delay", "never"]) {
      act(() => { fireEvent.change(openMode(), { target: { value: mode } }); });
      rerender();
      const hintId = openMode().getAttribute("aria-describedby")!;
      descriptions.add(document.getElementById(hintId)!.textContent!);
    }
    expect(descriptions.size).toBe(3);
  });
});
