import { act, cleanup, fireEvent, render, renderHook, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { SettingsHarness } from "./test/settingsHarness";
import { readNoteSyntaxMode, useNoteSyntaxMode, writeNoteSyntaxMode } from "./noteSyntaxMode";

describe("Notes settings", () => {
  beforeEach(() => localStorage.clear());
  afterEach(cleanup);

  it("starts on Hide and keeps the Markdown Syntax choice on this device", async () => {
    render(<SettingsHarness initialSection="notes" />);
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    const group = within(dialog).getByRole("radiogroup", { name: "Markdown syntax" });
    expect(within(group).getByRole("radio", { name: "Hide" })).toBeChecked();
    expect(group).toHaveAccessibleDescription();
    fireEvent.click(within(group).getByRole("radio", { name: "Hybrid" }));
    expect(within(group).getByRole("radio", { name: "Hybrid" })).toBeChecked();
    expect(readNoteSyntaxMode()).toBe("hybrid");
  });

  it("tells an open editor when the choice changes, and ignores an unknown stored value", () => {
    localStorage.setItem("threestrands.notes.syntax", "sideways");
    const { result } = renderHook(() => useNoteSyntaxMode());
    expect(result.current).toBe("hide");
    act(() => writeNoteSyntaxMode("show"));
    expect(result.current).toBe("show");
  });
});
