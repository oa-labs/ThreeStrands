import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { MeetingRecording, TranscriptionModelStatus } from "./domain";
import { EventRecordings } from "./EventRecordings";
import { RecordingContext } from "./recordingContext";
import { recordingController } from "./test/recordingController";
import type { NoteReference } from "./noteEntities";

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

const occurrence: NoteReference = { version: 1, kind: "event", id: "evt", account: "me@example.com", calendar: "work" };
const row = (overrides: Partial<MeetingRecording> = {}): MeetingRecording => ({
  id: "r1", eventRef: occurrence, title: "Weekly Sync", startedAt: "2026-11-27T15:00:00Z", endedAt: "2026-11-27T15:42:00Z",
  durationMs: 42 * 60_000, state: "recorded", error: null, audioDeleted: false, transcriptionPercent: null,
  tracks: [
    { track: "mic", state: "active", deviceName: "Mic", firstFrameOffsetMs: 0, error: null },
    { track: "system", state: "active", deviceName: null, firstFrameOffsetMs: 0, error: null },
  ],
  ...overrides,
});
const renderSection = (controller = recordingController()) =>
  render(<RecordingContext.Provider value={controller}><EventRecordings occurrence={occurrence} /></RecordingContext.Provider>);

describe("EventRecordings", () => {
  beforeEach(() => { vi.restoreAllMocks(); bus.handlers.clear(); });
  afterEach(cleanup);

  it("is left out when the meeting has no recordings", async () => {
    const list = vi.spyOn(mailClient, "listRecordings").mockResolvedValue([]);
    const { container } = renderSection();
    await waitFor(() => expect(list).toHaveBeenCalledWith(occurrence));
    expect(container).toBeEmptyDOMElement();
  });

  it("reads nothing where recording is unsupported", () => {
    const list = vi.spyOn(mailClient, "listRecordings");
    const { container } = render(<EventRecordings occurrence={occurrence} />);
    expect(list).not.toHaveBeenCalled();
    expect(container).toBeEmptyDOMElement();
  });

  it("lists one row per recording with its state, a date, and an audio glyph", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([
      row(),
      row({ id: "r2", state: "interrupted", durationMs: 18 * 60_000 }),
      row({ id: "r3", state: "recording", durationMs: 0, endedAt: null }),
      row({ id: "r4", state: "failed" }),
    ]);
    const { container } = renderSection();
    const section = await screen.findByRole("region", { name: "Recording" });
    expect(within(section).getAllByText("Recording", { selector: ".context-row-title" })).toHaveLength(4);
    const details = [...section.querySelectorAll(".context-row-detail")].map((node) => node.textContent);
    expect(details).toEqual(["42 min · Waiting to transcribe", "Interrupted · 18 min saved", "Recording…", "Transcription failed"]);
    expect(section.querySelectorAll(".context-row-date")).toHaveLength(4);
    expect(container.querySelector("[data-attachment-kind='audio']")).not.toBeNull();
  });

  it("says when only the microphone was recorded", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ tracks: [
      { track: "mic", state: "active", deviceName: "Mic", firstFrameOffsetMs: 0, error: null },
      { track: "system", state: "unavailable", deviceName: null, firstFrameOffsetMs: null, error: null },
    ] })]);
    renderSection();
    expect(await screen.findByText("42 min · Waiting to transcribe · Microphone only")).toBeInTheDocument();
  });

  it("offers no Delete on a recording still in progress", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ state: "recording", endedAt: null })]);
    renderSection();
    await screen.findByText("Recording…");
    expect(screen.queryByRole("button", { name: "Delete Recording" })).not.toBeInTheDocument();
  });

  it("asks before deleting, deletes only when confirmed, and then shows the rest", async () => {
    const list = vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row()]);
    const remove = vi.spyOn(mailClient, "deleteRecording").mockResolvedValue(undefined);
    renderSection();
    fireEvent.click(await screen.findByRole("button", { name: "Delete Recording" }));
    const confirm = screen.getByRole("group", { name: "Confirm Delete Recording" });
    await waitFor(() => expect(within(confirm).getByRole("button", { name: "Cancel" })).toHaveFocus());
    expect(remove).not.toHaveBeenCalled();
    fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(remove).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Delete Recording" }));
    list.mockResolvedValue([]);
    fireEvent.click(within(screen.getByRole("group", { name: "Confirm Delete Recording" })).getByRole("button", { name: "Delete Recording" }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith("r1"));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Recording" })).not.toBeInTheDocument());
  });

  it("keeps the recording and says so when deleting fails", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row()]);
    vi.spyOn(mailClient, "deleteRecording").mockRejectedValue(new Error("Recording is in use"));
    renderSection();
    fireEvent.click(await screen.findByRole("button", { name: "Delete Recording" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Confirm Delete Recording" })).getByRole("button", { name: "Delete Recording" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Recording is in use");
    expect(screen.getByRole("region", { name: "Recording" })).toBeInTheDocument();
  });

  it("reads the rows again when the recorder says they changed", async () => {
    const list = vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ state: "recording", endedAt: null })]);
    const view = renderSection();
    await screen.findByText("Recording…");
    list.mockResolvedValue([row()]);
    view.rerender(<RecordingContext.Provider value={recordingController({ recordingsVersion: 1 })}><EventRecordings occurrence={occurrence} /></RecordingContext.Provider>);
    expect(await screen.findByText("42 min · Waiting to transcribe")).toBeInTheDocument();
  });

  const model = (state: TranscriptionModelStatus["state"]): TranscriptionModelStatus => ({ model: "fast", bytes: 190_000_000, state, receivedBytes: 0, error: null });
  const detailOf = (section: HTMLElement) => [...section.querySelectorAll(".context-row-detail")].map((node) => node.textContent);

  it("says waiting for a model until the chosen model is ready, then waiting to transcribe", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ durationMs: 18 * 60_000 })]);
    const view = renderSection(recordingController({ transcriptionModel: model("absent") }));
    expect(await screen.findByText("18 min · Waiting for a transcription model")).toBeInTheDocument();
    view.rerender(<RecordingContext.Provider value={recordingController({ transcriptionModel: model("ready") })}><EventRecordings occurrence={occurrence} /></RecordingContext.Provider>);
    expect(await screen.findByText("18 min · Waiting to transcribe")).toBeInTheDocument();
  });

  it("shows the transcription percent and follows transcription-progress", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ state: "transcribing", transcriptionPercent: 40 }), row({ id: "r2", state: "transcribing", transcriptionPercent: 10 })]);
    renderSection();
    const section = await screen.findByRole("region", { name: "Recording" });
    expect(detailOf(section)).toEqual(["Transcribing… 40%", "Transcribing… 10%"]);
    await waitFor(() => expect(bus.handlers.get("transcription-progress")?.size).toBeGreaterThan(0));
    act(() => bus.emit("transcription-progress", { recordingId: "r1", percent: 41 }));
    expect(detailOf(section)).toEqual(["Transcribing… 41%", "Transcribing… 10%"]);
  });

  it("offers Show Transcript only when transcribed, and Retry Transcription only for a failed recording with audio", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([
      row({ id: "t", state: "transcribed" }),
      row({ id: "f", state: "failed" }),
      row({ id: "g", state: "failed", audioDeleted: true }),
      row({ id: "p", state: "recorded" }),
      row({ id: "i", state: "interrupted" }),
    ]);
    renderSection();
    await screen.findByRole("region", { name: "Recording" });
    expect(screen.getAllByRole("button", { name: "Show Transcript" })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Retry Transcription" })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Delete Recording" })).toHaveLength(5);
  });

  it("retries a failed transcription and reads the rows again", async () => {
    const list = vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ state: "failed" })]);
    const transcribe = vi.spyOn(mailClient, "transcribeRecording").mockResolvedValue(undefined);
    renderSection();
    list.mockResolvedValue([row({ state: "recorded" })]);
    fireEvent.click(await screen.findByRole("button", { name: "Retry Transcription" }));
    await waitFor(() => expect(transcribe).toHaveBeenCalledExactlyOnceWith("r1"));
    expect(await screen.findByText("42 min · Waiting to transcribe")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry Transcription" })).not.toBeInTheDocument();
  });

  it("says why a retry was refused and leaves the row", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ state: "failed" })]);
    vi.spyOn(mailClient, "transcribeRecording").mockRejectedValue(new Error("The audio is gone."));
    renderSection();
    fireEvent.click(await screen.findByRole("button", { name: "Retry Transcription" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The audio is gone.");
    expect(screen.getByRole("button", { name: "Retry Transcription" })).toBeInTheDocument();
  });

  it("opens the transcript from Show Transcript and stays open while the rows are read again", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ state: "transcribed" })]);
    vi.spyOn(mailClient, "recordingTranscript").mockResolvedValue({ recordingId: "r1", speakerLabels: true, lines: [{ startMs: 0, endMs: 1000, speaker: "you", text: "Hello" }] });
    const view = renderSection();
    fireEvent.click(await screen.findByRole("button", { name: "Show Transcript" }));
    expect(await screen.findByRole("dialog", { name: "Transcript" })).toBeInTheDocument();
    view.rerender(<RecordingContext.Provider value={recordingController({ recordingsVersion: 1 })}><EventRecordings occurrence={occurrence} /></RecordingContext.Provider>);
    expect(await screen.findByText("Hello")).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Transcript" })).toBeInTheDocument();
  });

  it("closes the transcript and drops the row after Delete Recording from it", async () => {
    const list = vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ state: "transcribed" })]);
    vi.spyOn(mailClient, "recordingTranscript").mockResolvedValue({ recordingId: "r1", speakerLabels: false, lines: [] });
    const remove = vi.spyOn(mailClient, "deleteRecording").mockResolvedValue(undefined);
    renderSection();
    fireEvent.click(await screen.findByRole("button", { name: "Show Transcript" }));
    const dialog = await screen.findByRole("dialog", { name: "Transcript" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete Recording" }));
    list.mockResolvedValue([]);
    fireEvent.click(within(within(dialog).getByRole("group", { name: "Confirm Delete Recording" })).getByRole("button", { name: "Delete Recording" }));
    await waitFor(() => expect(remove).toHaveBeenCalledExactlyOnceWith("r1"));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Transcript" })).not.toBeInTheDocument());
    await waitFor(() => expect(screen.queryByRole("region", { name: "Recording" })).not.toBeInTheDocument());
  });
});
