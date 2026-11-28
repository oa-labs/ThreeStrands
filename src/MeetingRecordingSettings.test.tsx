import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { AudioInputDevice, RecordingCapabilities, RecordingSettings, TranscriptionModelStatus } from "./domain";
import type { RecordingEvents } from "./recordingEvents";
import { MeetingRecordingSettings } from "./MeetingRecordingSettings";

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

const available: RecordingCapabilities = { supported: true, systemAudio: { kind: "available" }, speechAnalyzer: false };
const defaults: RecordingSettings = {
  recordMeetings: false, microphoneDeviceUid: null, transcriptionEngine: "auto", transcriptionModel: "fast", transcriptionLanguage: null,
  transcribeDuringMeetings: true, keepAudio: "days7", offerToRecord: true, showUpcomingMeetingsInTray: true, trayNextMeetingTitle: "titleAndTime", remindToTell: true,
  summarizeWithAi: false, includeMyNotes: false, attachTranscript: false, consentAcknowledged: false,
  customVocabulary: [], cloudTranscriptionEnabled: false, cloudTranscriptionDisclosureAcknowledged: false, cloudTranscriptionDefault: false,
  cloudTranscriptionProvider: null, cloudTranscriptionModel: "microsoft/mai-transcribe-2", cloudTranscriptionEndpoint: null, cloudTranscriptionDiarize: true,
};
const builtInMic: AudioInputDevice = { uid: "built-in", name: "MacBook Pro Microphone", isDefault: true, isBluetooth: false };

const model = (name: "fast" | "accurate", overrides: Partial<TranscriptionModelStatus> = {}): TranscriptionModelStatus => ({
  model: name, bytes: name === "fast" ? 190_000_000 : 574_000_000, state: "absent", receivedBytes: 0, error: null, ...overrides,
});

function setup(
  capabilities = available,
  saved = defaults,
  usage = { recordings: 3, audioBytes: 5 * 1024 * 1024 },
  models = [model("fast"), model("accurate")],
  devices: AudioInputDevice[] = [builtInMic],
) {
  vi.spyOn(mailClient, "transcriptionModels").mockResolvedValue(models);
  vi.spyOn(mailClient, "recordingSettings").mockResolvedValue(saved);
  vi.spyOn(mailClient, "recordingsUsage").mockResolvedValue(usage);
  vi.spyOn(mailClient, "recordingInputDevices").mockResolvedValue(devices);
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
    setup({ supported: true, systemAudio: { kind: "needsMacos", major: 13, minor: 6 }, speechAnalyzer: false });
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

describe("Transcription Model", () => {
  beforeEach(() => { vi.restoreAllMocks(); bus.handlers.clear(); });
  afterEach(cleanup);

  const pushModels = async (models: TranscriptionModelStatus[]) => {
    await waitFor(() => expect(bus.handlers.get("transcription-models-changed")?.size).toBeGreaterThan(0));
    act(() => bus.emit("transcription-models-changed" satisfies keyof RecordingEvents, models));
  };
  const group = async (name: string) => within(await screen.findByRole("group", { name }));

  it("chooses Fast or Accurate, saves the choice, and describes the field", async () => {
    const { save } = setup();
    const field = await screen.findByRole("combobox", { name: "Transcription Model" });
    expect(field).toHaveValue("fast");
    expect(field).toHaveAccessibleDescription();
    expect(within(field).getAllByRole("option").map((option) => option.textContent)).toEqual(["Fast", "Accurate"]);
    fireEvent.change(field, { target: { value: "accurate" } });
    await waitFor(() => expect(save).toHaveBeenCalledWith({ ...defaults, transcriptionModel: "accurate" }));
  });

  it("turns Transcribe During Meetings off and on, describing what it costs", async () => {
    const { save } = setup();
    const toggle = await screen.findByRole("checkbox", { name: "Transcribe During Meetings" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).toBeChecked();
    expect(toggle).toHaveAccessibleDescription();
    fireEvent.click(toggle);
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, transcribeDuringMeetings: false }));
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, transcribeDuringMeetings: true }));
  });

  it("shows Transcribe During Meetings off when it was saved off", async () => {
    setup(available, { ...defaults, transcribeDuringMeetings: false });
    const toggle = await screen.findByRole("checkbox", { name: "Transcribe During Meetings" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).not.toBeChecked();
  });

  it("shows each model's size, language note and state, and downloads nothing until Download is pressed", async () => {
    setup();
    const download = vi.spyOn(mailClient, "downloadTranscriptionModel").mockResolvedValue(undefined);
    const fast = await group("Fast model");
    await waitFor(() => expect(fast.getByText("190 MB, English only")).toBeInTheDocument());
    expect(fast.getByText("Not downloaded")).toBeInTheDocument();
    const accurate = await group("Accurate model");
    expect(accurate.getByText("574 MB, many languages")).toBeInTheDocument();
    expect(download).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("combobox", { name: "Transcription Model" }), { target: { value: "accurate" } });
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Transcription Model" })).toHaveValue("accurate"));
    expect(download).not.toHaveBeenCalled();
    fireEvent.click(accurate.getByRole("button", { name: "Download" }));
    await waitFor(() => expect(download).toHaveBeenCalledExactlyOnceWith("accurate"));
  });

  it("offers Download for a partial download and says how far it got", async () => {
    setup(available, defaults, undefined, [model("fast", { receivedBytes: 95_000_000 }), model("accurate")]);
    const fast = await group("Fast model");
    await waitFor(() => expect(fast.getByText("Partly downloaded, 50%")).toBeInTheDocument());
    expect(fast.getByRole("button", { name: "Download" })).toBeInTheDocument();
  });

  it("shows progress with Cancel while downloading, updating from the event", async () => {
    setup(available, defaults, undefined, [model("fast", { state: "downloading", receivedBytes: 19_000_000 }), model("accurate")]);
    const cancel = vi.spyOn(mailClient, "cancelTranscriptionModelDownload").mockResolvedValue(undefined);
    const fast = await group("Fast model");
    await waitFor(() => expect(fast.getByText("Downloading… 10%")).toBeInTheDocument());
    expect(fast.getByRole("progressbar", { name: "Fast model download" })).toHaveValue(10);
    expect(fast.queryByRole("button", { name: "Download" })).not.toBeInTheDocument();
    await pushModels([model("fast", { state: "downloading", receivedBytes: 95_000_000 }), model("accurate")]);
    expect(fast.getByText("Downloading… 50%")).toBeInTheDocument();
    expect(fast.getByRole("progressbar", { name: "Fast model download" })).toHaveValue(50);
    fireEvent.click(fast.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(cancel).toHaveBeenCalledExactlyOnceWith("fast"));
  });

  it("shows checking while verifying, with no way to cancel", async () => {
    setup(available, defaults, undefined, [model("fast", { state: "verifying", receivedBytes: 190_000_000 }), model("accurate")]);
    const fast = await group("Fast model");
    await waitFor(() => expect(fast.getByText("Checking the download…")).toBeInTheDocument());
    expect(fast.queryByRole("button")).not.toBeInTheDocument();
  });

  it("shows the failure with Try Again, which downloads again", async () => {
    setup(available, defaults, undefined, [model("fast", { state: "failed", error: "Not enough free disk space to download the model." }), model("accurate")]);
    const download = vi.spyOn(mailClient, "downloadTranscriptionModel").mockResolvedValue(undefined);
    const fast = await group("Fast model");
    expect(await fast.findByRole("alert")).toHaveTextContent("Not enough free disk space to download the model.");
    fireEvent.click(fast.getByRole("button", { name: "Try Again" }));
    await waitFor(() => expect(download).toHaveBeenCalledExactlyOnceWith("fast"));
  });

  it("asks before deleting a downloaded model, and deletes only when confirmed", async () => {
    setup(available, defaults, undefined, [model("fast", { state: "ready", receivedBytes: 190_000_000 }), model("accurate")]);
    const remove = vi.spyOn(mailClient, "deleteTranscriptionModel").mockResolvedValue(undefined);
    const fast = await group("Fast model");
    await waitFor(() => expect(fast.getByText("Downloaded")).toBeInTheDocument());
    fireEvent.click(fast.getByRole("button", { name: "Delete Model" }));
    const confirm = screen.getByRole("group", { name: "Confirm Delete Model" });
    await waitFor(() => expect(within(confirm).getByRole("button", { name: "Cancel" })).toHaveFocus());
    expect(remove).not.toHaveBeenCalled();
    fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(remove).not.toHaveBeenCalled();
    fireEvent.click(fast.getByRole("button", { name: "Delete Model" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Confirm Delete Model" })).getByRole("button", { name: "Delete Model" }));
    await waitFor(() => expect(remove).toHaveBeenCalledExactlyOnceWith("fast"));
    await waitFor(() => expect(screen.queryByRole("group", { name: "Confirm Delete Model" })).not.toBeInTheDocument());
  });

  it("says why a command failed", async () => {
    setup();
    vi.spyOn(mailClient, "downloadTranscriptionModel").mockRejectedValue(new Error("This model is already downloading."));
    const fast = await group("Fast model");
    fireEvent.click(await fast.findByRole("button", { name: "Download" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("already downloading");
  });

  it("lists microphones, saves the choice, and warns about Bluetooth headsets", async () => {
    const airpods: AudioInputDevice = { uid: "bt-airpods", name: "AirPods Pro", isDefault: false, isBluetooth: true };
    const { save } = setup(available, defaults, undefined, undefined, [builtInMic, airpods]);
    const field = await screen.findByRole("combobox", { name: "Microphone" });
    await waitFor(() => expect(field).toBeEnabled());
    expect(field).toHaveValue("");
    expect(field).toHaveAccessibleDescription();
    expect(within(field).getAllByRole("option").map((o) => o.textContent)).toEqual(["System Default", "MacBook Pro Microphone (Default)", "AirPods Pro"]);
    expect(screen.queryByRole("note")).not.toBeInTheDocument();

    fireEvent.change(field, { target: { value: "bt-airpods" } });
    await waitFor(() => expect(save).toHaveBeenCalledWith({ ...defaults, microphoneDeviceUid: "bt-airpods" }));
    expect(await screen.findByRole("note")).toHaveTextContent("Bluetooth headsets");
  });

  it("offers Apple On-Device first when SpeechAnalyzer is available, and lets the user switch to Whisper", async () => {
    const { save } = setup({ ...available, speechAnalyzer: true });
    const field = await screen.findByRole("combobox", { name: "Transcription Model" });
    await waitFor(() => expect(field).toBeEnabled());
    expect(field).toHaveValue("apple");
    expect(within(field).getAllByRole("option").map((o) => o.textContent)).toEqual(["Apple On-Device", "Fast", "Accurate"]);

    fireEvent.change(field, { target: { value: "accurate" } });
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, transcriptionEngine: "whisper", transcriptionModel: "accurate" }));
    fireEvent.change(field, { target: { value: "apple" } });
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, transcriptionEngine: "speechAnalyzer", transcriptionModel: "accurate" }));
  });

  it("saves Transcription Language and returns null for Auto", async () => {
    const { save } = setup();
    const field = await screen.findByRole("combobox", { name: "Transcription Language" });
    await waitFor(() => expect(field).toBeEnabled());
    expect(field).toHaveValue("");
    expect(field).toHaveAccessibleDescription();
    fireEvent.change(field, { target: { value: "es" } });
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, transcriptionLanguage: "es" }));
    fireEvent.change(field, { target: { value: "" } });
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, transcriptionLanguage: null }));
  });

  it("saves Summarize with AI as it changes and attaches its help text", async () => {
    const { save } = setup();
    const toggle = await screen.findByRole("checkbox", { name: "Summarize with AI" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).not.toBeChecked();
    expect(toggle).toHaveAccessibleDescription();
    fireEvent.click(toggle);
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, summarizeWithAi: true }));
    expect(toggle).toBeChecked();
  });

  it("saves Offer to Record Meetings when toggled and attaches its help text", async () => {
    const { save } = setup();
    const toggle = await screen.findByRole("checkbox", { name: "Offer to Record Meetings" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).toBeChecked();
    expect(toggle).toHaveAccessibleDescription();
    fireEvent.click(toggle);
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, offerToRecord: false }));
    expect(toggle).not.toBeChecked();
  });

  it("saves Show Upcoming Meetings in Menu Bar and Menu Bar Countdown mode", async () => {
    const { save } = setup();
    const upcomingToggle = await screen.findByRole("checkbox", { name: "Show Upcoming Meetings in Menu Bar" });
    const countdownSelect = await screen.findByRole("combobox", { name: "Menu Bar Countdown" });
    await waitFor(() => expect(upcomingToggle).toBeEnabled());
    expect(upcomingToggle).toBeChecked();
    expect(upcomingToggle).toHaveAccessibleDescription();
    expect(countdownSelect).toBeEnabled();
    expect(countdownSelect).toHaveValue("titleAndTime");
    expect(countdownSelect).toHaveAccessibleDescription();

    fireEvent.change(countdownSelect, { target: { value: "timeOnly" } });
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, trayNextMeetingTitle: "timeOnly" }));

    fireEvent.click(upcomingToggle);
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({
      ...defaults,
      showUpcomingMeetingsInTray: false,
      trayNextMeetingTitle: "timeOnly",
    }));
    expect(countdownSelect).toBeDisabled();
  });

  it("lets the user add, review Sent-mail suggestions, and remove Custom Vocabulary without auto-saving suggestions", async () => {
    const { save } = setup();
    const suggest = vi.spyOn(mailClient, "suggestRecordingVocabulary").mockResolvedValue([
      { term: "ThreeStrands", threadCount: 5 },
      { term: "QBR", threadCount: 3 },
    ]);
    const input = await screen.findByRole("textbox", { name: "Custom Vocabulary term" });
    await waitFor(() => expect(input).toBeEnabled());

    fireEvent.change(input, { target: { value: "CoreAudio" } });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({ ...defaults, customVocabulary: ["CoreAudio"] }));

    fireEvent.click(screen.getByRole("button", { name: "Suggest from Sent Mail" }));
    const panel = await screen.findByRole("region", { name: "Sent Mail Vocabulary Suggestions" });
    expect(suggest).toHaveBeenCalledOnce();
    expect(save).toHaveBeenCalledTimes(1);

    // Uncheck QBR and confirm only ThreeStrands is added alongside CoreAudio.
    fireEvent.click(within(panel).getByRole("checkbox", { name: /QBR/ }));
    fireEvent.click(within(panel).getByRole("button", { name: "Add Selected" }));
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({
      ...defaults,
      customVocabulary: ["CoreAudio", "ThreeStrands"],
    }));

    fireEvent.click(screen.getByRole("button", { name: "Remove CoreAudio" }));
    await waitFor(() => expect(save).toHaveBeenLastCalledWith({
      ...defaults,
      customVocabulary: ["ThreeStrands"],
    }));
  });

  it("requires privacy disclosure confirmation before enabling Cloud Transcription", async () => {
    localStorage.setItem("threestrands.settings.ai.provider", "openrouter");
    try {
      const { save } = setup();
      const toggle = await screen.findByRole("checkbox", { name: "Cloud Transcription" });
      await waitFor(() => expect(toggle).toBeEnabled());
      expect(toggle).not.toBeChecked();
      expect(toggle).toHaveAccessibleDescription();

      fireEvent.click(toggle);
      const confirm = screen.getByRole("group", { name: "Confirm Cloud Transcription" });
      await waitFor(() => expect(within(confirm).getByRole("button", { name: "Cancel" })).toHaveFocus());
      expect(save).not.toHaveBeenCalled();

      fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
      expect(toggle).not.toBeChecked();
      expect(save).not.toHaveBeenCalled();

      fireEvent.click(toggle);
      fireEvent.click(within(screen.getByRole("group", { name: "Confirm Cloud Transcription" })).getByRole("button", { name: "Enable Cloud Transcription" }));
      await waitFor(() => expect(save).toHaveBeenLastCalledWith({
        ...defaults,
        cloudTranscriptionEnabled: true,
        cloudTranscriptionDisclosureAcknowledged: true,
        cloudTranscriptionProvider: "openrouter",
        cloudTranscriptionModel: "microsoft/mai-transcribe-2",
      }));
      expect(await screen.findByRole("textbox", { name: "Cloud Transcription Model" })).toHaveValue("microsoft/mai-transcribe-2");
    } finally {
      localStorage.removeItem("threestrands.settings.ai.provider");
    }
  });
});
