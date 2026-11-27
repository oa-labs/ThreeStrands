import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { RecordingCapabilities, RecordingSettings } from "./domain";
import { MeetingRecordingSettings } from "./MeetingRecordingSettings";

const available: RecordingCapabilities = { supported: true, systemAudio: { kind: "available" } };
const defaults: RecordingSettings = {
  recordMeetings: false, transcriptionModel: "fast", keepAudio: "days7", offerToRecord: true, remindToTell: true,
  summarizeWithAi: false, includeMyNotes: false, attachTranscript: false, consentAcknowledged: false,
};

function setup(capabilities = available, saved = defaults, usage = { recordings: 3, audioBytes: 5 * 1024 * 1024 }) {
  vi.spyOn(mailClient, "recordingSettings").mockResolvedValue(saved);
  vi.spyOn(mailClient, "recordingsUsage").mockResolvedValue(usage);
  const save = vi.spyOn(mailClient, "setRecordingSettings").mockResolvedValue(undefined);
  render(<MeetingRecordingSettings capabilities={capabilities} />);
  return { save };
}

describe("MeetingRecordingSettings", () => {
  beforeEach(() => vi.restoreAllMocks());
  afterEach(cleanup);

  it("asks for the microphone when Record Meetings is turned on, and saves once it is granted", async () => {
    const { save } = setup();
    const request = vi.spyOn(mailClient, "requestRecordingPermissions").mockResolvedValue({ microphone: "granted" });
    const toggle = await screen.findByRole("checkbox", { name: "Record Meetings" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(request).not.toHaveBeenCalled();
    fireEvent.click(toggle);
    await waitFor(() => expect(save).toHaveBeenCalledWith({ ...defaults, recordMeetings: true }));
    expect(request).toHaveBeenCalledOnce();
    expect(toggle).toBeChecked();
  });

  it("keeps the switch off and points to System Settings when the microphone is denied", async () => {
    const { save } = setup();
    vi.spyOn(mailClient, "requestRecordingPermissions").mockResolvedValue({ microphone: "denied" });
    const toggle = await screen.findByRole("checkbox", { name: "Record Meetings" });
    await waitFor(() => expect(toggle).toBeEnabled());
    fireEvent.click(toggle);
    expect(await screen.findByRole("alert")).toHaveTextContent("System Settings → Privacy & Security → Microphone");
    expect(toggle).not.toBeChecked();
    expect(save).not.toHaveBeenCalled();
  });

  it("explains a restricted microphone", async () => {
    setup();
    vi.spyOn(mailClient, "requestRecordingPermissions").mockResolvedValue({ microphone: "restricted" });
    const toggle = await screen.findByRole("checkbox", { name: "Record Meetings" });
    await waitFor(() => expect(toggle).toBeEnabled());
    fireEvent.click(toggle);
    expect(await screen.findByRole("alert")).toHaveTextContent("restricted");
    expect(toggle).not.toBeChecked();
  });

  it("turns recording off without asking for anything", async () => {
    const { save } = setup(available, { ...defaults, recordMeetings: true });
    const request = vi.spyOn(mailClient, "requestRecordingPermissions");
    const toggle = await screen.findByRole("checkbox", { name: "Record Meetings" });
    await waitFor(() => expect(toggle).toBeChecked());
    fireEvent.click(toggle);
    await waitFor(() => expect(save).toHaveBeenCalledWith({ ...defaults, recordMeetings: false }));
    expect(request).not.toHaveBeenCalled();
  });

  it("saves Keep Audio and the reminder as they change", async () => {
    const { save } = setup();
    fireEvent.change(await screen.findByRole("combobox", { name: "Keep Audio" }), { target: { value: "untilDeleted" } });
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, keepAudio: "untilDeleted" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Remind Me to Tell Participants" }));
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, keepAudio: "untilDeleted", remindToTell: false }));
    const options = within(screen.getByRole("combobox", { name: "Keep Audio" })).getAllByRole("option").map((option) => option.textContent);
    expect(options).toEqual(["7 Days", "30 Days", "Until Deleted", "Until Transcribed"]);
  });

  it("attaches its help text to the controls", async () => {
    setup();
    expect(await screen.findByRole("checkbox", { name: "Record Meetings" })).toHaveAccessibleDescription();
    expect(screen.getByRole("combobox", { name: "Keep Audio" })).toHaveAccessibleDescription();
    expect(screen.getByRole("checkbox", { name: "Remind Me to Tell Participants" })).toHaveAccessibleDescription();
  });

  it("says system audio needs macOS 14.4 and that recordings will be microphone only", async () => {
    setup({ supported: true, systemAudio: { kind: "needsMacos", major: 13, minor: 6 } });
    const note = await screen.findByRole("note");
    expect(note).toHaveTextContent("macOS 14.4");
    expect(note).toHaveTextContent("microphone only");
  });

  it("shows no system audio notice when it is available", async () => {
    setup();
    await screen.findByRole("checkbox", { name: "Record Meetings" });
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("shows the space used, and nothing to delete when there are no recordings", async () => {
    setup(available, defaults, { recordings: 0, audioBytes: 0 });
    expect(await screen.findByText("No recordings.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete All Audio" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Delete All Recordings" })).toBeDisabled();
  });

  it("asks before deleting all audio, and deletes nothing on Cancel", async () => {
    setup();
    const deleteAudio = vi.spyOn(mailClient, "deleteAllRecordingAudio").mockResolvedValue(undefined);
    expect(await screen.findByText("3 recordings, 5.0 MB of audio.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Delete All Audio" }));
    const confirm = screen.getByRole("group", { name: "Confirm Delete All Audio" });
    await waitFor(() => expect(within(confirm).getByRole("button", { name: "Cancel" })).toHaveFocus());
    expect(deleteAudio).not.toHaveBeenCalled();
    fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("group", { name: "Confirm Delete All Audio" })).not.toBeInTheDocument();
    expect(deleteAudio).not.toHaveBeenCalled();
  });

  it("deletes all audio when confirmed, then reads the usage again", async () => {
    setup();
    const deleteAudio = vi.spyOn(mailClient, "deleteAllRecordingAudio").mockResolvedValue(undefined);
    await screen.findByText("3 recordings, 5.0 MB of audio.");
    vi.mocked(mailClient.recordingsUsage).mockResolvedValue({ recordings: 3, audioBytes: 0 });
    fireEvent.click(screen.getByRole("button", { name: "Delete All Audio" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Confirm Delete All Audio" })).getByRole("button", { name: "Delete All Audio" }));
    await waitFor(() => expect(deleteAudio).toHaveBeenCalledOnce());
    expect(await screen.findByText("3 recordings, 0 B of audio.")).toBeInTheDocument();
  });

  it("asks before deleting all recordings, then deletes them", async () => {
    setup();
    const deleteAll = vi.spyOn(mailClient, "deleteAllRecordings").mockResolvedValue(undefined);
    await screen.findByText("3 recordings, 5.0 MB of audio.");
    fireEvent.click(screen.getByRole("button", { name: "Delete All Recordings" }));
    const confirm = screen.getByRole("group", { name: "Confirm Delete All Recordings" });
    expect(deleteAll).not.toHaveBeenCalled();
    vi.mocked(mailClient.recordingsUsage).mockResolvedValue({ recordings: 0, audioBytes: 0 });
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete All Recordings" }));
    await waitFor(() => expect(deleteAll).toHaveBeenCalledOnce());
    expect(await screen.findByText("No recordings.")).toBeInTheDocument();
  });

  it("shows a failed delete and keeps the recordings listed", async () => {
    setup();
    vi.spyOn(mailClient, "deleteAllRecordings").mockRejectedValue(new Error("Disk is read-only"));
    await screen.findByText("3 recordings, 5.0 MB of audio.");
    fireEvent.click(screen.getByRole("button", { name: "Delete All Recordings" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Confirm Delete All Recordings" })).getByRole("button", { name: "Delete All Recordings" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Disk is read-only");
    expect(screen.getByText("3 recordings, 5.0 MB of audio.")).toBeInTheDocument();
  });
});
