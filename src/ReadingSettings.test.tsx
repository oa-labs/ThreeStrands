import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { DEFAULT_AUTO_READ_DELAY_SECONDS } from "./settings";
import { SettingsHarness } from "./test/settingsHarness";

async function openReading() {
  render(<SettingsHarness initialSection="reading" />);
  return screen.findByRole("dialog", { name: "Settings" });
}

const openMode = (dialog: HTMLElement) => within(dialog).getByRole("combobox", { name: "Mark Read After Opening" });
const delay = (dialog: HTMLElement) => within(dialog).queryByRole("spinbutton", { name: "Mark Read Delay in Seconds" });
const stored = (key: string) => localStorage.getItem(`threestrands.settings.${key}`);

describe("Reading & Triage settings", () => {
  beforeEach(() => localStorage.clear());
  afterEach(cleanup);

  describe("Mark Read After Opening", () => {
    it("starts on After a Delay with the default delay, and describes the choice", async () => {
      const dialog = await openReading();
      expect(openMode(dialog)).toHaveValue("delay");
      expect(delay(dialog)).toHaveValue(DEFAULT_AUTO_READ_DELAY_SECONDS);
      expect(openMode(dialog)).toHaveAccessibleDescription();
      expect(delay(dialog)).toHaveAccessibleDescription();
    });

    it("stores Immediately as a zero delay and hides the delay field", async () => {
      const dialog = await openReading();
      fireEvent.change(openMode(dialog), { target: { value: "immediately" } });
      expect(stored("autoReadDelaySeconds")).toBe("0");
      expect(openMode(dialog)).toHaveValue("immediately");
      expect(delay(dialog)).not.toBeInTheDocument();
    });

    it("returns from Immediately to the default delay", async () => {
      localStorage.setItem("threestrands.settings.autoReadDelaySeconds", "0");
      const dialog = await openReading();
      expect(openMode(dialog)).toHaveValue("immediately");
      fireEvent.change(openMode(dialog), { target: { value: "delay" } });
      expect(delay(dialog)).toHaveValue(DEFAULT_AUTO_READ_DELAY_SECONDS);
      expect(stored("autoReadDelaySeconds")).toBe(String(DEFAULT_AUTO_READ_DELAY_SECONDS));
    });

    it("keeps the chosen delay through Never and back", async () => {
      localStorage.setItem("threestrands.settings.autoReadDelaySeconds", "15");
      const dialog = await openReading();
      fireEvent.change(openMode(dialog), { target: { value: "never" } });
      expect(stored("markReadOnOpen")).toBe("false");
      expect(openMode(dialog)).toHaveValue("never");
      expect(delay(dialog)).not.toBeInTheDocument();

      fireEvent.change(openMode(dialog), { target: { value: "delay" } });
      expect(stored("markReadOnOpen")).toBe("true");
      expect(delay(dialog)).toHaveValue(15);
    });

    it("ignores a zero typed into the delay field instead of switching to Immediately", async () => {
      const dialog = await openReading();
      fireEvent.change(delay(dialog)!, { target: { value: "0" } });
      expect(openMode(dialog)).toHaveValue("delay");
      fireEvent.change(delay(dialog)!, { target: { value: "9" } });
      expect(stored("autoReadDelaySeconds")).toBe("9");
    });

    it("describes the effect of each choice differently", async () => {
      const dialog = await openReading();
      const descriptions = new Set<string>();
      for (const mode of ["immediately", "delay", "never"]) {
        fireEvent.change(openMode(dialog), { target: { value: mode } });
        const hintId = openMode(dialog).getAttribute("aria-describedby")!;
        descriptions.add(document.getElementById(hintId)!.textContent!);
      }
      expect(descriptions.size).toBe(3);
    });
  });

  it.each([
    ["Mark Read When Archiving", "markReadOnArchive"],
    ["Mark Read When Moving to Trash", "markReadOnTrash"],
    ["Mark Read When Snoozing", "markReadOnSnooze"],
    ["Return Snoozed Conversations as Unread", "snoozeReturnsUnread"],
  ])("%s starts off, is described, and saves", async (name, key) => {
    const dialog = await openReading();
    const toggle = within(dialog).getByRole("checkbox", { name });
    expect(toggle).not.toBeChecked();
    expect(toggle).toHaveAccessibleDescription();
    fireEvent.click(toggle);
    expect(toggle).toBeChecked();
    expect(stored(key)).toBe("true");
  });

  it("chooses what opens after archiving or moving to Trash", async () => {
    const dialog = await openReading();
    const after = within(dialog).getByRole("combobox", { name: "After Archiving or Moving to Trash" });
    expect(after).toHaveValue("older");
    expect(after).toHaveAccessibleDescription();
    for (const value of ["newer", "list", "older"]) {
      fireEvent.change(after, { target: { value } });
      expect(after).toHaveValue(value);
      expect(stored("afterTriage")).toBe(value);
    }
  });
});
