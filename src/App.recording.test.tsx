import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";
import type { RecordingSettings } from "./domain";
import type { RecordingEvents } from "./recordingEvents";

// Slim wiring checks: the palette, the menu bar, and the quit prompt reach the
// recorder. Behavior of each piece is tested where it lives.
const bus = vi.hoisted(() => {
  const handlers = new Map<string, Set<(payload: unknown) => void>>();
  return { handlers, emit(name: string, payload: unknown) { for (const handler of handlers.get(name) ?? []) handler(payload); } };
});
vi.mock("./recordingEvents", async (original) => ({
  ...(await original<typeof import("./recordingEvents")>()),
  listenForRecordingEvent: (name: string, handler: (payload: unknown) => void) => {
    const set = bus.handlers.get(name) ?? new Set();
    set.add(handler);
    bus.handlers.set(name, set);
    return () => set.delete(handler);
  },
}));

const settings = (overrides: Partial<RecordingSettings> = {}): RecordingSettings => ({
  recordMeetings: true, microphoneDeviceUid: null, transcriptionEngine: "auto", transcriptionModel: "fast", transcriptionLanguage: null, transcribeDuringMeetings: true, keepAudio: "days7", offerToRecord: true, remindToTell: true,
  summarizeWithAi: false, includeMyNotes: false, attachTranscript: false, consentAcknowledged: true, ...overrides,
});

async function fire<K extends keyof RecordingEvents>(name: K, payload: RecordingEvents[K]) {
  await waitFor(() => expect(bus.handlers.get(name)?.size).toBeGreaterThan(0));
  act(() => bus.emit(name, payload));
}

describe("App meeting recording wiring", () => {
  beforeEach(async () => {
    localStorage.clear();
    bus.handlers.clear();
    await mailClient.stopRecording();
  });
  afterEach(async () => {
    cleanup();
    vi.restoreAllMocks();
    await mailClient.stopRecording();
    await mailClient.setRecordingSettings(settings({ recordMeetings: false, consentAcknowledged: false }));
  });

  it("starts an ad-hoc recording from the palette, shows the indicator and the reminder, and stops from the indicator", async () => {
    await mailClient.setRecordingSettings(settings());
    const start = vi.spyOn(mailClient, "startRecording");
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Command Palette (Ctrl+K)" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "Filter Commands" }), { target: { value: "Start Recording" } });
    const command = await screen.findByRole("button", { name: /Start Recording/ });
    await waitFor(() => expect(command).toBeEnabled());
    fireEvent.click(command);
    await waitFor(() => expect(start).toHaveBeenCalledWith({ eventRef: null, title: expect.stringMatching(/^Meeting at / ) }));
    expect(await screen.findByRole("group", { name: "Recording" })).toBeInTheDocument();
    expect(await screen.findByText("Let everyone know this meeting is being recorded.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Stop Recording" }));
    await waitFor(() => expect(screen.queryByRole("group", { name: "Recording" })).not.toBeInTheDocument());
  });

  it("offers no recording commands while Record Meetings is off", async () => {
    await mailClient.setRecordingSettings(settings({ recordMeetings: false }));
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Command Palette (Ctrl+K)" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "Filter Commands" }), { target: { value: "Recording" } });
    expect(await screen.findByRole("button", { name: /Start Recording/ })).toBeDisabled();
  });

  it("runs the consent flow when the menu bar starts a recording for the first time", async () => {
    await mailClient.setRecordingSettings(settings({ consentAcknowledged: false }));
    const start = vi.spyOn(mailClient, "startRecording");
    render(<App />);
    await screen.findByRole("button", { name: "Command Palette (Ctrl+K)" });
    await waitFor(() => expect(bus.handlers.get("recording-tray-command")?.size).toBeGreaterThan(0));
    await fire("recording-tray-command", { command: "start" });
    const dialog = await screen.findByRole("dialog", { name: "Recording Meetings" });
    expect(start).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Start Recording" }));
    await waitFor(() => expect(start).toHaveBeenCalledOnce());
    expect(await mailClient.recordingSettings()).toMatchObject({ consentAcknowledged: true });
  });

  it("opens Settings → Meeting Recording from the menu bar", async () => {
    await mailClient.setRecordingSettings(settings());
    render(<App />);
    await screen.findByRole("button", { name: "Command Palette (Ctrl+K)" });
    await fire("recording-tray-command", { command: "openSettings" });
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(await within(dialog).findByRole("region", { name: "Meeting Recording" })).toBeInTheDocument();
  });

  it("asks before quitting while recording, and stops only when confirmed", async () => {
    await mailClient.setRecordingSettings(settings());
    const confirm = vi.spyOn(mailClient, "confirmRecordingQuit");
    render(<App />);
    await screen.findByRole("button", { name: "Command Palette (Ctrl+K)" });
    await fire("recording-quit-requested", null);
    const dialog = await screen.findByRole("dialog", { name: "Stop Recording and Quit?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog", { name: "Stop Recording and Quit?" })).not.toBeInTheDocument();
    expect(confirm).not.toHaveBeenCalled();
    await fire("recording-quit-requested", null);
    fireEvent.click(within(await screen.findByRole("dialog", { name: "Stop Recording and Quit?" })).getByRole("button", { name: "Stop and Quit" }));
    await waitFor(() => expect(confirm).toHaveBeenCalledOnce());
  });

  it("opens the Review Follow-through modal from the menu bar and from Review Last Recording", async () => {
    await mailClient.setRecordingSettings(settings());
    const transcribed = {
      id: "rec-latest",
      eventRef: null,
      title: "Quick Chat",
      startedAt: "2026-11-27T14:00:00Z",
      endedAt: "2026-11-27T14:20:00Z",
      durationMs: 20 * 60_000,
      state: "transcribed" as const,
      error: null,
      audioDeleted: false,
      transcriptionPercent: null,
      tracks: [],
      hasRecap: false,
      reviewedOperationId: null,
    };
    vi.spyOn(mailClient, "latestTranscribedRecording").mockResolvedValue(transcribed);
    vi.spyOn(mailClient, "recordingReviewContext").mockResolvedValue({
      recording: transcribed,
      recap: null,
      transcriptBytes: 1200,
      overBudget: false,
      durationMs: 20 * 60_000,
      hasUserNotes: false,
      notesExcludedFromAi: false,
    });
    render(<App />);
    await screen.findByRole("button", { name: "Command Palette (Ctrl+K)" });
    await fire("recording-tray-command", { command: "review", recordingId: "rec-latest" });
    const trayDialog = await screen.findByRole("dialog", { name: "Review Follow-through" });
    expect(within(trayDialog).getByText(/Quick Chat/)).toBeInTheDocument();
    fireEvent.click(within(trayDialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Review Follow-through" })).not.toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Command Palette (Ctrl+K)" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "Filter Commands" }), { target: { value: "Review Last Recording" } });
    fireEvent.click(await screen.findByRole("button", { name: /Review Last Recording/ }));
    expect(await screen.findByRole("dialog", { name: "Review Follow-through" })).toBeInTheDocument();
  });

  it("starts an offered meeting recording from the menu bar offer command", async () => {
    await mailClient.setRecordingSettings(settings());
    const start = vi.spyOn(mailClient, "startRecording");
    render(<App />);
    await screen.findByRole("button", { name: "Command Palette (Ctrl+K)" });
    const startRequest = {
      eventRef: { version: 1 as const, kind: "event" as const, id: "evt-1", account: "me@example.com", calendar: "work" },
      title: "Weekly Sync",
      attendees: ["ann@example.org"],
    };
    await fire("recording-tray-command", { command: "startOffer", startRequest });
    await waitFor(() => expect(start).toHaveBeenCalledWith(startRequest));
    expect(await screen.findByRole("group", { name: "Recording" })).toBeInTheDocument();
  });
});
