import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import type { Account } from "./domain";
import { queuePortablePreferences } from "./syncedPreferences";
import { WritingProfileSettings } from "./WritingProfileSettings";
import { WRITING_PROFILE_LIMITS, readWritingProfiles, resetWritingProfilesForTests, setWritingProfile } from "./writingProfiles";

describe("Writing profile settings", () => {
  const accounts = [
    { email: "work@example.com", nickname: "Work" },
    { email: "home@example.com", nickname: null },
  ] as Account[];
  beforeEach(() => {
    localStorage.clear();
    resetWritingProfilesForTests();
    vi.clearAllMocks();
    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    vi.mocked(invoke).mockResolvedValue(undefined);
  });
  afterEach(cleanup);

  const renderSettings = (props: Partial<Parameters<typeof WritingProfileSettings>[0]> = {}) =>
    render(<WritingProfileSettings accounts={accounts} draftReviewOn onChange={queuePortablePreferences} {...props} />);

  function syncedProfiles() {
    const queued = vi.mocked(invoke).mock.calls.filter(([command]) => command === "update_synced_preferences");
    return (queued.at(-1)?.[1] as { preferences: { writingProfiles: unknown } } | undefined)?.preferences.writingProfiles;
  }

  it("saves each account's profile on blur and queues it for sync", async () => {
    renderSettings();
    const about = screen.getByRole("textbox", { name: "About You" });
    fireEvent.change(about, { target: { value: "CEO of a consulting firm." } });
    expect(readWritingProfiles()).toEqual({});
    fireEvent.blur(about);
    expect(readWritingProfiles()).toEqual({ "work@example.com": { aboutYou: "CEO of a consulting firm.", reviewInstructions: "" } });
    await waitFor(() => expect(syncedProfiles()).toEqual(readWritingProfiles()));

    fireEvent.change(screen.getByRole("combobox", { name: "Account" }), { target: { value: "home@example.com" } });
    expect(screen.getByRole("textbox", { name: "About You" })).toHaveValue("");
    const instructions = screen.getByRole("textbox", { name: "Review Instructions" });
    fireEvent.change(instructions, { target: { value: "Keep it warm." } });
    fireEvent.blur(instructions);
    expect(readWritingProfiles()).toEqual({
      "work@example.com": { aboutYou: "CEO of a consulting firm.", reviewInstructions: "" },
      "home@example.com": { aboutYou: "", reviewInstructions: "Keep it warm." },
    });

    fireEvent.change(screen.getByRole("combobox", { name: "Account" }), { target: { value: "work@example.com" } });
    expect(screen.getByRole("textbox", { name: "About You" })).toHaveValue("CEO of a consulting firm.");
  });

  it("keeps an edit when the account changes or Settings closes before blur, and clears an emptied profile", () => {
    setWritingProfile("work@example.com", { aboutYou: "CEO", reviewInstructions: "" });
    const view = renderSettings();
    fireEvent.change(screen.getByRole("textbox", { name: "About You" }), { target: { value: "" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Account" }), { target: { value: "home@example.com" } });
    expect(readWritingProfiles()).toEqual({});
    fireEvent.change(screen.getByRole("textbox", { name: "About You" }), { target: { value: "Parent" } });
    view.unmount();
    expect(readWritingProfiles()).toEqual({ "home@example.com": { aboutYou: "Parent", reviewInstructions: "" } });
  });

  it("gives each field room for long text, limits it to what Draft Review accepts, and counts characters", () => {
    renderSettings({ accounts: accounts.slice(0, 1) });
    expect(screen.queryByRole("combobox", { name: "Account" })).not.toBeInTheDocument();
    const about = screen.getByRole("textbox", { name: "About You" });
    const instructions = screen.getByRole("textbox", { name: "Review Instructions" });
    expect(about).toHaveAttribute("maxLength", String(WRITING_PROFILE_LIMITS.aboutYou));
    expect(instructions).toHaveAttribute("maxLength", String(WRITING_PROFILE_LIMITS.reviewInstructions));
    expect(Number(about.getAttribute("rows"))).toBeGreaterThanOrEqual(8);
    expect(Number(instructions.getAttribute("rows"))).toBeGreaterThanOrEqual(6);
    expect(screen.getAllByText("0 / 2,000")).toHaveLength(2);
    fireEvent.change(about, { target: { value: "Café owner" } });
    expect(screen.getByText("10 / 2,000")).toBeInTheDocument();
  });

  it("stays editable while Draft Review is off and links to AI Provider", () => {
    setWritingProfile("work@example.com", { aboutYou: "CEO", reviewInstructions: "" });
    const onOpenAiSettings = vi.fn();
    renderSettings({ draftReviewOn: false, onOpenAiSettings });
    expect(screen.getByRole("note")).toHaveTextContent("Draft Review is off");
    expect(screen.getByRole("textbox", { name: "About You" })).toHaveValue("CEO");
    fireEvent.click(screen.getByRole("button", { name: "AI Provider" }));
    expect(onOpenAiSettings).toHaveBeenCalledOnce();
    cleanup();
    renderSettings();
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("asks for a mail account when none is connected", () => {
    renderSettings({ accounts: [] });
    expect(screen.getByText(/Connect a mail account/)).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "About You" })).not.toBeInTheDocument();
  });
});
