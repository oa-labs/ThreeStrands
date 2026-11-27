import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { MeetingRecording, RecordingTranscript } from "./domain";
import { TranscriptDialog } from "./TranscriptDialog";

const recording: MeetingRecording = {
  id: "r1", eventRef: null, title: "Weekly Sync", startedAt: "2026-11-27T15:00:00Z", endedAt: "2026-11-27T16:10:00Z",
  durationMs: 70 * 60_000, state: "transcribed", error: null, audioDeleted: false, transcriptionPercent: null, tracks: [],
};
const line = (startMs: number, speaker: "you" | "others" | null, text: string) => ({ startMs, endMs: startMs + 1000, speaker, text });
const labelled: RecordingTranscript = { recordingId: "r1", speakerLabels: true, lines: [line(754_000, "you", "Hello"), line(3_723_000, "others", "Hi there")] };

function open(transcript: RecordingTranscript | Error = labelled) {
  const read = vi.spyOn(mailClient, "recordingTranscript");
  if (transcript instanceof Error) read.mockRejectedValue(transcript); else read.mockResolvedValue(transcript);
  const onClose = vi.fn();
  const onDeleted = vi.fn();
  const opener = document.createElement("button");
  opener.textContent = "Show Transcript";
  document.body.append(opener);
  opener.focus();
  const view = render(<TranscriptDialog recording={recording} onClose={onClose} onDeleted={onDeleted} />);
  return { onClose, onDeleted, opener, view };
}

describe("TranscriptDialog", () => {
  beforeEach(() => vi.restoreAllMocks());
  afterEach(() => { cleanup(); document.body.querySelectorAll(":scope > button").forEach((node) => node.remove()); });

  it("is a modal-scope dialog titled Transcript, with timestamps and You or Others labels", async () => {
    open();
    const dialog = await screen.findByRole("dialog", { name: "Transcript" });
    expect(dialog).toHaveAttribute("data-shortcut-scope", "modal");
    expect(dialog).toHaveAttribute("aria-modal", "true");
    const lines = [...dialog.querySelectorAll(".transcript-line")].map((node) => node.textContent);
    expect(lines).toEqual(["12:34YouHello", "1:02:03OthersHi there"]);
  });

  it("shows no You or Others labels when the transcript has none", async () => {
    open({ recordingId: "r1", speakerLabels: false, lines: [line(0, null, "Only the microphone"), line(5000, null, "heard this")] });
    const dialog = await screen.findByRole("dialog", { name: "Transcript" });
    await within(dialog).findByText("Only the microphone");
    expect(dialog.querySelector(".transcript-line-speaker")).toBeNull();
    expect(within(dialog).queryByText("You")).not.toBeInTheDocument();
    expect(within(dialog).queryByText("Others")).not.toBeInTheDocument();
  });

  it("has no editable control", async () => {
    open();
    const dialog = await screen.findByRole("dialog", { name: "Transcript" });
    await within(dialog).findByText("Hello");
    expect(within(dialog).queryByRole("textbox")).not.toBeInTheDocument();
    expect(dialog.querySelector("[contenteditable], input, textarea")).toBeNull();
  });

  it("renders transcript text containing HTML as text only", async () => {
    open({ recordingId: "r1", speakerLabels: false, lines: [line(0, null, '<img src=x onerror="alert(1)"><b>bold</b> <script>1</script>')] });
    const dialog = await screen.findByRole("dialog", { name: "Transcript" });
    expect(await within(dialog).findByText('<img src=x onerror="alert(1)"><b>bold</b> <script>1</script>')).toBeInTheDocument();
    expect(dialog.querySelector("img, b, script")).toBeNull();
  });

  it("copies one line per transcript line", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText: write }, configurable: true });
    open();
    await screen.findByText("Hello");
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(write).toHaveBeenCalledExactlyOnceWith("[12:34] You: Hello\n[1:02:03] Others: Hi there"));
    expect(await screen.findByRole("status")).toHaveTextContent("Copied");
  });

  it("says when copying fails", async () => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: vi.fn().mockRejectedValue(new Error("Clipboard is blocked.")) }, configurable: true });
    open();
    await screen.findByText("Hello");
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Clipboard is blocked.");
  });

  it("says why the transcript could not be read, with nothing to copy", async () => {
    open(new Error("This recording has no transcript."));
    expect(await screen.findByRole("alert")).toHaveTextContent("This recording has no transcript.");
    expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
  });

  it("asks before deleting, deletes only when confirmed, then reports it", async () => {
    const remove = vi.spyOn(mailClient, "deleteRecording").mockResolvedValue(undefined);
    const { onDeleted } = open();
    await screen.findByText("Hello");
    fireEvent.click(screen.getByRole("button", { name: "Delete Recording" }));
    const confirm = screen.getByRole("group", { name: "Confirm Delete Recording" });
    await waitFor(() => expect(within(confirm).getByRole("button", { name: "Cancel" })).toHaveFocus());
    expect(remove).not.toHaveBeenCalled();
    fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(remove).not.toHaveBeenCalled();
    expect(onDeleted).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Delete Recording" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Confirm Delete Recording" })).getByRole("button", { name: "Delete Recording" }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledOnce());
    expect(remove).toHaveBeenCalledExactlyOnceWith("r1");
  });

  it("stays open and says why when deleting fails", async () => {
    vi.spyOn(mailClient, "deleteRecording").mockRejectedValue(new Error("Recording is in use"));
    const { onDeleted } = open();
    await screen.findByText("Hello");
    fireEvent.click(screen.getByRole("button", { name: "Delete Recording" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Confirm Delete Recording" })).getByRole("button", { name: "Delete Recording" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Recording is in use");
    expect(onDeleted).not.toHaveBeenCalled();
  });

  it("starts focus on the transcript, traps Tab, makes the background inert, and Escape closes and restores focus", async () => {
    const { onClose, opener, view } = open();
    const dialog = await screen.findByRole("dialog", { name: "Transcript" });
    const text = within(dialog).getByRole("region", { name: "Transcript text" });
    await waitFor(() => expect(text).toHaveFocus());
    expect(opener.inert).toBe(true);
    const buttons = within(dialog).getAllByRole("button");
    const last = buttons.at(-1)!;
    last.focus();
    fireEvent.keyDown(last, { key: "Tab" });
    expect(buttons[0]).toHaveFocus();
    fireEvent.keyDown(buttons[0], { key: "Tab", shiftKey: true });
    expect(last).toHaveFocus();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
    view.unmount();
    expect(opener).toHaveFocus();
    expect(opener.inert).toBeFalsy();
  });

  it("scrolls long transcripts inside the dialog", async () => {
    open({ recordingId: "r1", speakerLabels: false, lines: Array.from({ length: 300 }, (_, index) => line(index * 1000, null, `Line ${index}`)) });
    const dialog = await screen.findByRole("dialog", { name: "Transcript" });
    expect(await within(dialog).findByText("Line 299")).toBeInTheDocument();
    expect(within(dialog).getByRole("region", { name: "Transcript text" })).toHaveClass("transcript-lines");
    expect(within(dialog).getAllByRole("button", { name: "Close" })).toHaveLength(2);
  });
});
