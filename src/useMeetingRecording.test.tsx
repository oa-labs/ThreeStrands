import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { RecordingSettings, RecordingStatus } from "./domain";
import type { RecordingEvents } from "./recordingEvents";
import { RECORDING_REMINDER } from "./recordingPolicy";
import { useMeetingRecording } from "./useMeetingRecording";

const bus = vi.hoisted(() => {
  const handlers = new Map<string, Set<(payload: unknown) => void>>();
  return {
    handlers,
    emit(name: string, payload: unknown) { for (const handler of handlers.get(name) ?? []) handler(payload); },
  };
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
const emit = <K extends keyof RecordingEvents>(name: K, payload: RecordingEvents[K]) => act(() => bus.emit(name, payload));

const settings = (overrides: Partial<RecordingSettings> = {}): RecordingSettings => ({
  recordMeetings: true, transcriptionModel: "fast", transcribeDuringMeetings: true, keepAudio: "days7", offerToRecord: true, remindToTell: true,
  summarizeWithAi: false, includeMyNotes: false, attachTranscript: false, consentAcknowledged: true, ...overrides,
});
const status = (overrides: Partial<RecordingStatus> = {}): RecordingStatus => ({
  state: "recording", recordingId: "r1", title: "Meeting at 3:04 PM", eventRef: null, elapsedMs: 0, tracks: [],
  systemAudio: { kind: "available" }, lastStopReason: null, ...overrides,
});
const idle = status({ state: "idle", recordingId: null, title: null });

function setup(saved: RecordingSettings = settings(), supported = true) {
  vi.spyOn(mailClient, "recordingCapabilities").mockResolvedValue({ supported, systemAudio: { kind: "available" } });
  vi.spyOn(mailClient, "recordingStatus").mockResolvedValue(idle);
  vi.spyOn(mailClient, "recordingSettings").mockResolvedValue(saved);
  const setSettings = vi.spyOn(mailClient, "setRecordingSettings").mockResolvedValue(undefined);
  const startRecording = vi.spyOn(mailClient, "startRecording").mockResolvedValue(status());
  const onNotice = vi.fn();
  const onOpenSettings = vi.fn();
  const hook = renderHook(() => useMeetingRecording({ onNotice, onOpenSettings }));
  return { ...hook, onNotice, onOpenSettings, setSettings, startRecording };
}

describe("useMeetingRecording", () => {
  beforeEach(() => bus.handlers.clear());
  afterEach(() => vi.restoreAllMocks());

  it("is unsupported and offers nothing where the device cannot record", async () => {
    const { result } = setup(settings(), false);
    await waitFor(() => expect(result.current.capabilities).not.toBeNull());
    expect(result.current.supported).toBe(false);
    expect(result.current.enabled).toBe(false);
    expect(bus.handlers.size).toBe(0);
    expect(mailClient.recordingStatus).not.toHaveBeenCalled();
  });

  it("is enabled only when Record Meetings is on", async () => {
    const off = setup(settings({ recordMeetings: false }));
    await waitFor(() => expect(off.result.current.supported).toBe(true));
    await waitFor(() => expect(mailClient.recordingSettings).toHaveBeenCalled());
    expect(off.result.current.enabled).toBe(false);
    off.unmount();
    const on = setup();
    await waitFor(() => expect(on.result.current.enabled).toBe(true));
  });

  it("starts an ad-hoc recording with no event and reminds participants", async () => {
    const { result, startRecording, onNotice } = setup();
    await waitFor(() => expect(result.current.enabled).toBe(true));
    await act(() => result.current.startAdHoc());
    expect(startRecording).toHaveBeenCalledWith({ eventRef: null, title: expect.stringMatching(/^Meeting at \d{1,2}:\d{2}/) });
    expect(onNotice).toHaveBeenCalledWith({ message: RECORDING_REMINDER });
    expect(result.current.status.state).toBe("recording");
  });

  it("shows no reminder when Remind Me to Tell Participants is off", async () => {
    const { result, startRecording, onNotice } = setup(settings({ remindToTell: false }));
    await waitFor(() => expect(result.current.enabled).toBe(true));
    await act(() => result.current.startAdHoc());
    expect(startRecording).toHaveBeenCalled();
    expect(onNotice).not.toHaveBeenCalled();
  });

  it("asks for consent before the first start, and starts only once it is accepted", async () => {
    const { result, startRecording, setSettings, onNotice } = setup(settings({ consentAcknowledged: false }));
    await waitFor(() => expect(result.current.enabled).toBe(true));
    await act(() => result.current.startAdHoc());
    expect(result.current.consentPending).toBe(true);
    expect(startRecording).not.toHaveBeenCalled();

    await act(() => result.current.acceptConsent());
    expect(setSettings).toHaveBeenCalledWith(expect.objectContaining({ consentAcknowledged: true, recordMeetings: true }));
    expect(startRecording).toHaveBeenCalledOnce();
    expect(result.current.consentPending).toBe(false);
    expect(onNotice).toHaveBeenCalledWith({ message: RECORDING_REMINDER });
  });

  it("starts nothing when consent is cancelled", async () => {
    const { result, startRecording, setSettings } = setup(settings({ consentAcknowledged: false }));
    await waitFor(() => expect(result.current.enabled).toBe(true));
    await act(() => result.current.startAdHoc());
    act(() => result.current.cancelConsent());
    expect(result.current.consentPending).toBe(false);
    expect(startRecording).not.toHaveBeenCalled();
    expect(setSettings).not.toHaveBeenCalled();
  });

  it("opens Settings instead of starting while Record Meetings is off", async () => {
    const { result, startRecording, onOpenSettings } = setup(settings({ recordMeetings: false }));
    await waitFor(() => expect(result.current.supported).toBe(true));
    await act(() => result.current.startAdHoc());
    expect(onOpenSettings).toHaveBeenCalledOnce();
    expect(startRecording).not.toHaveBeenCalled();
  });

  it("reports a failed start", async () => {
    const { result, startRecording, onNotice } = setup();
    startRecording.mockRejectedValue(new Error("Microphone is busy"));
    await waitFor(() => expect(result.current.enabled).toBe(true));
    await act(() => result.current.startAdHoc());
    expect(onNotice).toHaveBeenCalledWith({ message: "Microphone is busy" });
    expect(result.current.status.state).toBe("idle");
  });

  it("follows the recorder's state events, and reads Recording rows again when one ends", async () => {
    const { result } = setup();
    await waitFor(() => expect(result.current.supported).toBe(true));
    await waitFor(() => expect(bus.handlers.get("recording-state")?.size).toBe(1));
    emit("recording-state", status({ elapsedMs: 5000 }));
    expect(result.current.status).toMatchObject({ state: "recording", elapsedMs: 5000 });
    const version = result.current.recordingsVersion;
    emit("recording-state", idle);
    expect(result.current.recordingsVersion).toBe(version + 1);
    emit("recordings-changed", { recordingId: "r1" });
    expect(result.current.recordingsVersion).toBe(version + 2);
  });

  it("explains a recording the app ended on its own", async () => {
    const { result, onNotice } = setup();
    await waitFor(() => expect(bus.handlers.get("recording-state")?.size).toBe(1));
    emit("recording-state", status());
    emit("recording-state", { ...idle, lastStopReason: "lowDisk" });
    expect(onNotice).toHaveBeenCalledWith({ message: expect.stringContaining("disk space") });
    expect(result.current.status.state).toBe("idle");
  });

  it("pauses, resumes, and stops through the recorder", async () => {
    const { result } = setup();
    await waitFor(() => expect(result.current.enabled).toBe(true));
    vi.spyOn(mailClient, "pauseRecording").mockResolvedValue(status({ state: "paused" }));
    vi.spyOn(mailClient, "resumeRecording").mockResolvedValue(status());
    vi.spyOn(mailClient, "stopRecording").mockResolvedValue(idle);
    await act(() => result.current.pause());
    expect(result.current.status.state).toBe("paused");
    await act(() => result.current.resume());
    expect(result.current.status.state).toBe("recording");
    await act(() => result.current.stop());
    expect(result.current.status.state).toBe("idle");
  });

  it("handles menu bar commands: start runs the start flow, openSettings opens Settings", async () => {
    const { result, startRecording, onOpenSettings } = setup(settings({ consentAcknowledged: false }));
    await waitFor(() => expect(bus.handlers.get("recording-tray-command")?.size).toBe(1));
    emit("recording-tray-command", { command: "openSettings" });
    expect(onOpenSettings).toHaveBeenCalledOnce();
    emit("recording-tray-command", { command: "start" });
    await waitFor(() => expect(result.current.consentPending).toBe(true));
    expect(startRecording).not.toHaveBeenCalled();
  });

  it("asks before quitting, and stops the recording only when confirmed", async () => {
    const { result } = setup();
    const confirm = vi.spyOn(mailClient, "confirmRecordingQuit").mockResolvedValue(undefined);
    await waitFor(() => expect(bus.handlers.get("recording-quit-requested")?.size).toBe(1));
    emit("recording-quit-requested", null);
    expect(result.current.quitRequested).toBe(true);
    act(() => result.current.cancelQuit());
    expect(result.current.quitRequested).toBe(false);
    expect(confirm).not.toHaveBeenCalled();
    emit("recording-quit-requested", null);
    await act(() => result.current.confirmQuit());
    expect(confirm).toHaveBeenCalledOnce();
    expect(result.current.quitRequested).toBe(false);
  });

  it("reads the settings again when Settings saves them", async () => {
    const { result } = setup(settings({ recordMeetings: false }));
    await waitFor(() => expect(result.current.supported).toBe(true));
    vi.mocked(mailClient.recordingSettings).mockResolvedValue(settings());
    const { announceRecordingSettingsChanged } = await import("./recordingEvents");
    act(() => announceRecordingSettingsChanged());
    await waitFor(() => expect(result.current.enabled).toBe(true));
  });
});
