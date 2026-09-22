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

function answerRequestedWords(transform: (word: string) => string = (word) => word) {
  for (const input of screen.getAllByRole("textbox")) {
    const label = input.closest("label")!.textContent!;
    const position = Number(/Word (\d+)/.exec(label)![1]) - 1;
    fireEvent.change(input, { target: { value: transform(WORDS[position]!) } });
  }
}

describe("pickConfirmationPositions", () => {
  it("returns distinct in-range positions in reading order", () => {
    for (let trial = 0; trial < 50; trial += 1) {
      const positions = dialog.pickConfirmationPositions(24);
      expect(positions).toHaveLength(dialog.RECOVERY_CONFIRMATION_WORDS);
      expect(new Set(positions).size).toBe(positions.length);
      expect(positions.every((position) => position >= 0 && position < 24)).toBe(true);
      expect([...positions].sort((a, b) => a - b)).toEqual(positions);
    }
  });

  it("never asks for more words than the phrase has", () => {
    expect(dialog.pickConfirmationPositions(2, 3, () => 0)).toEqual([0, 1]);
  });
});

describe("recoveryWordMatches", () => {
  it("ignores surrounding whitespace and case but not the word itself", () => {
    expect(dialog.recoveryWordMatches("absorb", "  Absorb ")).toBe(true);
    expect(dialog.recoveryWordMatches("absorb", "absurd")).toBe(false);
    expect(dialog.recoveryWordMatches("absorb", "")).toBe(false);
  });
});

describe("PendingRecoveryPhraseDialog", () => {
  it("renders nothing while no phrase is pending", () => {
    render(<dialog.PendingRecoveryPhraseDialog />);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows every word numbered and cannot be dismissed without confirming", () => {
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

  it("releases the phrase only after the requested words are re-entered correctly", () => {
    act(() => dialog.holdRecoveryPhrase(PHRASE));
    render(<dialog.PendingRecoveryPhraseDialog />);
    fireEvent.click(screen.getByRole("button", { name: "I’ve written it down" }));

    const finish = screen.getByRole("button", { name: "Finish" });
    expect(screen.getAllByRole("textbox")).toHaveLength(dialog.RECOVERY_CONFIRMATION_WORDS);
    expect(finish).toBeDisabled();

    answerRequestedWords((word) => `${word}x`);
    expect(finish).toBeDisabled();
    fireEvent.submit(finish.closest("form")!);
    expect(dialog.pendingRecoveryPhrase()).toBe(PHRASE);

    answerRequestedWords((word) => ` ${word.toUpperCase()} `);
    expect(finish).toBeEnabled();
    fireEvent.click(finish);
    expect(dialog.pendingRecoveryPhrase()).toBeNull();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("lets the user return to the phrase from the confirmation step", () => {
    act(() => dialog.holdRecoveryPhrase(PHRASE));
    render(<dialog.PendingRecoveryPhraseDialog />);
    fireEvent.click(screen.getByRole("button", { name: "I’ve written it down" }));
    fireEvent.click(screen.getByRole("button", { name: "Show phrase again" }));
    expect(screen.getByRole("list", { name: "Recovery phrase" })).toBeInTheDocument();
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
