import type { MeetingRecording, MicrophonePermission, RecordingCapabilities, RecordingStatus, RecordingStartRequest } from "../domain";
import { noteReferenceKey, type NoteReference } from "../noteEntities";

/** What the browser preview records into: in memory, with a fixed 1.4 MB per second of audio so Settings has sizes to show. */
const AUDIO_BYTES_PER_MS = 1.4;

const IDLE: RecordingStatus = {
  state: "idle", recordingId: null, title: null, eventRef: null, elapsedMs: 0, tracks: [],
  systemAudio: { kind: "available" }, lastStopReason: null,
};

/** In-memory equivalents of the native recording commands, for the browser preview and tests. */
export function createDemoRecordings(options: { microphone?: MicrophonePermission } = {}) {
  const recordings: MeetingRecording[] = [];
  let audio = new Set<string>();
  let counter = 0;
  let active: { id: string; request: RecordingStartRequest; startedAt: number; accrued: number; resumedAt: number | null } | null = null;
  const elapsed = () => active ? active.accrued + (active.resumedAt === null ? 0 : Date.now() - active.resumedAt) : 0;
  const status = (): RecordingStatus => active ? {
    ...IDLE,
    state: active.resumedAt === null ? "paused" : "recording",
    recordingId: active.id,
    title: active.request.title,
    eventRef: active.request.eventRef,
    elapsedMs: elapsed(),
    tracks: [{ track: "mic", state: "active" }, { track: "system", state: "active" }],
  } : { ...IDLE };
  const durationMs = (recording: MeetingRecording) => recording.durationMs;
  const api = {
    async recordingCapabilities(): Promise<RecordingCapabilities> { return { supported: true, systemAudio: { kind: "available" } }; },
    async recordingStatus() { return status(); },
    async startRecording(request: RecordingStartRequest) {
      if (active) throw new Error("A recording is already in progress");
      active = { id: `demo-recording-${++counter}`, request: structuredClone(request), startedAt: Date.now(), accrued: 0, resumedAt: Date.now() };
      return status();
    },
    async pauseRecording() {
      if (active?.resumedAt != null) { active.accrued = elapsed(); active.resumedAt = null; }
      return status();
    },
    async resumeRecording() {
      if (active && active.resumedAt === null) active.resumedAt = Date.now();
      return status();
    },
    async stopRecording() {
      if (active) {
        const duration = elapsed();
        recordings.unshift({
          id: active.id, eventRef: active.request.eventRef, title: active.request.title,
          startedAt: new Date(active.startedAt).toISOString(), endedAt: new Date().toISOString(), durationMs: duration, state: "recorded", error: null,
          tracks: [
            { track: "mic", state: "active", deviceName: "MacBook Pro Microphone", firstFrameOffsetMs: 0, error: null },
            { track: "system", state: "active", deviceName: null, firstFrameOffsetMs: 0, error: null },
          ],
        });
        audio.add(active.id);
        active = null;
      }
      return status();
    },
    async listRecordings(eventRef: NoteReference) {
      const key = noteReferenceKey(eventRef);
      return structuredClone(recordings.filter((recording) => recording.eventRef && noteReferenceKey(recording.eventRef) === key));
    },
    async deleteRecording(id: string) {
      const index = recordings.findIndex((recording) => recording.id === id);
      if (index >= 0) recordings.splice(index, 1);
      audio.delete(id);
    },
    async deleteRecordingAudio(id: string) { audio.delete(id); },
    async recordingsUsage() {
      const bytes = recordings.filter((recording) => audio.has(recording.id)).reduce((sum, recording) => sum + durationMs(recording) * AUDIO_BYTES_PER_MS, 0);
      return { recordings: recordings.length, audioBytes: Math.round(bytes) };
    },
    async deleteAllRecordingAudio() { audio = new Set(); },
    async deleteAllRecordings() { recordings.splice(0); audio = new Set(); },
    async requestRecordingPermissions() { return { microphone: options.microphone ?? "granted" as const }; },
    async confirmRecordingQuit() { await api.stopRecording(); },
  };
  return api;
}
