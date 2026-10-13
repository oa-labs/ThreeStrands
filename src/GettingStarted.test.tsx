import { openUrl } from "@tauri-apps/plugin-opener";
import { act, cleanup, fireEvent, render, renderHook, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { GettingStartedDialog, useGettingStarted } from "./GettingStarted";
import { GETTING_STARTED_STEPS, GETTING_STARTED_STORAGE_KEY, readGettingStarted } from "./gettingStartedSteps";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

const noFacts = { mailConnected: false, calendarConnected: false, aiReady: false };

beforeEach(() => localStorage.removeItem(GETTING_STARTED_STORAGE_KEY));
afterEach(() => {
  cleanup();
  vi.mocked(openUrl).mockClear();
});

describe("GettingStartedDialog", () => {
  it("shows progress, offers actions only for open steps, and marks optional ones", () => {
    const onAction = vi.fn();
    render(<GettingStartedDialog state={{ completed: ["connectMail"], dismissed: false }} onAction={onAction} onDismiss={vi.fn()} onClose={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Getting Started" });
    expect(within(dialog).getByText(`1 of ${GETTING_STARTED_STEPS.length} done`)).toBeInTheDocument();
    expect(within(dialog).getByRole("progressbar", { name: "Getting Started progress" })).toHaveAttribute("value", "1");

    const connect = within(dialog).getByRole("listitem", { name: "Connect a mail account, done" });
    expect(within(connect).queryByRole("button", { name: "Add Account" })).toBeNull();
    const calendar = within(dialog).getByRole("listitem", { name: "Connect a calendar" });
    expect(within(calendar).getByText("Optional")).toBeInTheDocument();

    fireEvent.click(within(calendar).getByRole("button", { name: "Connect Calendar" }));
    expect(onAction).toHaveBeenCalledWith("connectCalendar");
  });

  it("opens step docs and the docs home in the system browser", () => {
    render(<GettingStartedDialog state={{ completed: [], dismissed: false }} onAction={vi.fn()} onDismiss={vi.fn()} onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Learn More: Set up AI assistance" }));
    expect(openUrl).toHaveBeenLastCalledWith("https://www.threestrands.app/docs/ai.html");
    fireEvent.click(screen.getByRole("button", { name: "Read the Docs" }));
    expect(openUrl).toHaveBeenLastCalledWith("https://www.threestrands.app/docs/index.html");
  });

  it("hides the checklist on request", () => {
    const onDismiss = vi.fn();
    render(<GettingStartedDialog state={{ completed: [], dismissed: false }} onAction={vi.fn()} onDismiss={onDismiss} onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Hide Getting Started" }));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});

describe("useGettingStarted", () => {
  it("checks off fact-backed steps as they become true and keeps them after", () => {
    const { result, rerender } = renderHook((facts) => useGettingStarted(facts), { initialProps: noFacts });
    expect(result.current.remaining).toBe(GETTING_STARTED_STEPS.length);
    rerender({ ...noFacts, mailConnected: true });
    expect(result.current.state.completed).toEqual(["connectMail"]);
    rerender(noFacts);
    expect(result.current.state.completed).toEqual(["connectMail"]);
    expect(readGettingStarted().completed).toEqual(["connectMail"]);
  });

  it("records completed actions and dismissal on this device", () => {
    const { result } = renderHook(() => useGettingStarted(noFacts));
    act(() => result.current.complete("commandPalette"));
    act(() => result.current.dismiss());
    expect(result.current.visible).toBe(false);
    expect(readGettingStarted()).toEqual({ completed: ["commandPalette"], dismissed: true });
  });
});
