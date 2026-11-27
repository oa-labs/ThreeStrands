import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { MeetingRecording } from "./domain";
import { EventRecordings } from "./EventRecordings";
import { RecordingContext } from "./recordingContext";
import { recordingController } from "./test/recordingController";
import type { NoteReference } from "./noteEntities";

const occurrence: NoteReference = { version: 1, kind: "event", id: "evt", account: "me@example.com", calendar: "work" };
const row = (overrides: Partial<MeetingRecording> = {}): MeetingRecording => ({
  id: "r1", eventRef: occurrence, title: "Weekly Sync", startedAt: "2026-11-27T15:00:00Z", endedAt: "2026-11-27T15:42:00Z",
  durationMs: 42 * 60_000, state: "recorded", error: null,
  tracks: [
    { track: "mic", state: "active", deviceName: "Mic", firstFrameOffsetMs: 0, error: null },
    { track: "system", state: "active", deviceName: null, firstFrameOffsetMs: 0, error: null },
  ],
  ...overrides,
});
const renderSection = (controller = recordingController()) =>
  render(<RecordingContext.Provider value={controller}><EventRecordings occurrence={occurrence} /></RecordingContext.Provider>);

describe("EventRecordings", () => {
  beforeEach(() => vi.restoreAllMocks());
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
    expect(details).toEqual(["42 min · Recorded", "Interrupted · 18 min saved", "Recording…", "Failed"]);
    expect(section.querySelectorAll(".context-row-date")).toHaveLength(4);
    expect(container.querySelector("[data-attachment-kind='audio']")).not.toBeNull();
  });

  it("says when only the microphone was recorded", async () => {
    vi.spyOn(mailClient, "listRecordings").mockResolvedValue([row({ tracks: [
      { track: "mic", state: "active", deviceName: "Mic", firstFrameOffsetMs: 0, error: null },
      { track: "system", state: "unavailable", deviceName: null, firstFrameOffsetMs: null, error: null },
    ] })]);
    renderSection();
    expect(await screen.findByText("42 min · Recorded · Microphone only")).toBeInTheDocument();
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
    expect(await screen.findByText("42 min · Recorded")).toBeInTheDocument();
  });
});
