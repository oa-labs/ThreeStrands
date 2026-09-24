import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type DialogModule = typeof import("./RecoveryPhraseDialog");

const WORDS = [
  "abandon", "ability", "able", "about", "above", "absent", "absorb", "abstract",
  "absurd", "abuse", "access", "accident", "account", "accuse", "achieve", "acid",
  "acoustic", "acquire", "across", "act", "action", "actor", "actress", "actual",
];
const PHRASE = WORDS.join(" ");

// The holder is module-level state by design, so each test gets a fresh copy.
let dialog: DialogModule;
beforeEach(async () => {
  vi.resetModules();
  dialog = await import("./RecoveryPhraseDialog");
});
afterEach(cleanup);

describe("PendingRecoveryPhraseDialog", () => {
  it("renders nothing while no phrase is pending", () => {
    render(<dialog.PendingRecoveryPhraseDialog />);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows every word numbered and cannot be dismissed without acknowledging", () => {
    act(() => dialog.holdRecoveryPhrase(PHRASE));
    render(<dialog.PendingRecoveryPhraseDialog />);
    const modal = screen.getByRole("dialog", { name: "Save Your Recovery Phrase" });
    const items = within(screen.getByRole("list", { name: "Recovery phrase" })).getAllByRole("listitem");
    expect(items.map((item) => item.textContent)).toEqual(WORDS);

    expect(within(modal).queryByRole("button", { name: "Close" })).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.mouseDown(modal.parentElement!);
    expect(screen.getByRole("dialog", { name: "Save Your Recovery Phrase" })).toBeInTheDocument();
    expect(dialog.pendingRecoveryPhrase()).toBe(PHRASE);
  });

  it("survives the dialog unmounting, as when Settings changes section or closes", () => {
    act(() => dialog.holdRecoveryPhrase(PHRASE));
    const first = render(<dialog.PendingRecoveryPhraseDialog />);
    first.unmount();

    render(<dialog.PendingRecoveryPhraseDialog />);
    expect(screen.getByRole("list", { name: "Recovery phrase" })).toHaveTextContent(WORDS[0]!);
  });

  it("releases the phrase after the user acknowledges recording it without re-entering words", () => {
    act(() => dialog.holdRecoveryPhrase(PHRASE));
    render(<dialog.PendingRecoveryPhraseDialog />);
    expect(screen.queryAllByRole("textbox")).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "I’ve written it down" }));
    expect(dialog.pendingRecoveryPhrase()).toBeNull();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("copies the phrase and reports when the clipboard is unavailable", async () => {
    const writeText = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("denied"));
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    act(() => dialog.holdRecoveryPhrase(PHRASE));
    render(<dialog.PendingRecoveryPhraseDialog />);

    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(PHRASE);
    expect(await screen.findByText(/Copied\./)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(await screen.findByText(/Couldn’t copy/)).toBeInTheDocument();
  });
});
