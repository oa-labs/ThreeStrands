import type {
  AudioInputDevice, MeetingRecap, MeetingRecording, MicrophonePermission, RecapTimeRange,
  RecordingCapabilities, RecordingReviewContext, RecordingStartRequest, RecordingStatus,
  RecordingTranscript, TranscriptionModelName, TranscriptionModelStatus,
} from "../domain";
import { noteReferenceKey, type NoteReference } from "../noteEntities";

/** What the browser preview records into: in memory, with a fixed 1.4 MB per second of audio so Settings has sizes to show. */
const AUDIO_BYTES_PER_MS = 1.4;

/** Pinned sizes, matching the native model list. */
const MODEL_BYTES: Record<TranscriptionModelName, number> = { fast: 190_000_000, accurate: 574_000_000 };
/** How long a demo download takes from nothing to ready. */
const DEMO_DOWNLOAD_MS = 6_000;

const DEMO_LINES: { speaker: "you" | "others"; text: string }[] = [
  { speaker: "others", text: "Thanks for making time. Let's start with where the launch checklist stands." },
  { speaker: "you", text: "The release notes are drafted and QA signed off on Tuesday." },
  { speaker: "others", text: "Great. Anything blocking the rollout?" },
  { speaker: "you", text: "Only the migration dry run, which I'll finish today." },
  { speaker: "others", text: "Then let's confirm Thursday and send the announcement tomorrow." },
];

/** A believable transcript: lines every 20 seconds, with speakers only when `labels`. */
function demoTranscript(recordingId: string, durationMs: number, labels: boolean): RecordingTranscript {
  const step = Math.max(1000, Math.min(20_000, Math.floor(durationMs / DEMO_LINES.length)));
  return {
    recordingId,
    speakerLabels: labels,
    lines: DEMO_LINES.map((line, index) => ({ startMs: index * step, endMs: (index + 1) * step - 500, speaker: labels ? line.speaker : null, text: line.text })),
  };
}

const IDLE: RecordingStatus = {
  state: "idle", recordingId: null, title: null, eventRef: null, elapsedMs: 0, tracks: [],
  systemAudio: { kind: "available" }, systemAudioSilentWarning: false, lastStopReason: null, stillRecordingNudge: false,
};

/** In-memory equivalents of the native recording commands, for the browser preview and tests. */
export function createDemoRecordings(options: {
  microphone?: MicrophonePermission;
  speechAnalyzer?: boolean;
  /** The chosen transcription model; without it Fast is chosen. */
  chosenModel?: () => TranscriptionModelName;
  usingSpeechAnalyzer?: () => boolean;
  /** Meetings that start with a transcribed recording and a microphone-only one. */
  seed?: NoteReference[];
} = {}) {
  const recordings: MeetingRecording[] = [];
  const transcripts = new Map<string, RecordingTranscript>();
  const recaps = new Map<string, MeetingRecap>();
  const models: Record<TranscriptionModelName, { received: number; since: number | null; failed: string | null }> = {
    fast: { received: 0, since: null, failed: null }, accurate: { received: 0, since: null, failed: null },
  };
  const modelStatus = (model: TranscriptionModelName): TranscriptionModelStatus => {
    const entry = models[model];
    const bytes = MODEL_BYTES[model];
    if (entry.since !== null) {
      const received = Math.min(bytes, entry.received + Math.round(((Date.now() - entry.since) / DEMO_DOWNLOAD_MS) * bytes));
      if (received >= bytes) { entry.received = bytes; entry.since = null; }
      else return { model, bytes, state: "downloading", receivedBytes: received, error: null };
    }
    if (entry.received >= bytes) return { model, bytes, state: "ready", receivedBytes: bytes, error: null };
    return { model, bytes, state: entry.failed ? "failed" : "absent", receivedBytes: entry.received, error: entry.failed };
  };
  const track = (state: "active" | "unavailable") => [
    { track: "mic" as const, state: "active" as const, deviceName: "MacBook Pro Microphone", firstFrameOffsetMs: 0, error: null },
    { track: "system" as const, state, deviceName: null, firstFrameOffsetMs: state === "active" ? 0 : null, error: null },
  ];
  const transcribe = (recording: MeetingRecording) => {
    const labels = recording.tracks.some((entry) => entry.track === "system" && entry.state === "active");
    transcripts.set(recording.id, demoTranscript(recording.id, recording.durationMs, labels));
    recording.state = "transcribed";
  };
  /** A recording waiting for a model is picked up as soon as the chosen one is ready (or on-device SpeechAnalyzer). */
  const pickUp = () => {
    const ready = (options.usingSpeechAnalyzer?.() ?? false) || modelStatus(options.chosenModel?.() ?? "fast").state === "ready";
    if (!ready) return;
    for (const recording of recordings) if (recording.state === "recorded" && audio.has(recording.id)) transcribe(recording);
  };
  for (const [index, eventRef] of (options.seed ?? []).entries()) {
    for (const [offset, micOnly] of [false, true].entries()) {
      const id = `demo-seed-${index}-${offset}`;
      const startedAt = Date.now() - (index * 2 + offset + 1) * 86_400_000;
      const recording: MeetingRecording = {
        id, eventRef, title: "Weekly Sync", startedAt: new Date(startedAt).toISOString(), endedAt: new Date(startedAt + 42 * 60_000).toISOString(),
        durationMs: 42 * 60_000, state: "recorded", error: null, audioDeleted: false, transcriptionPercent: null, tracks: track(micOnly ? "unavailable" : "active"),
        hasRecap: false, reviewedOperationId: null,
      };
      recordings.push(recording);
      transcribe(recording);
    }
  }
  let audio = new Set<string>(recordings.map((recording) => recording.id));
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
    tracks: [{ track: "mic", state: "active", signalActive: true }, { track: "system", state: "active", signalActive: true }],
  } : { ...IDLE };
  const durationMs = (recording: MeetingRecording) => recording.durationMs;
  const api = {
    async recordingCapabilities(): Promise<RecordingCapabilities> { return { supported: true, systemAudio: { kind: "available" }, speechAnalyzer: options.speechAnalyzer ?? false }; },
    async recordingInputDevices(): Promise<AudioInputDevice[]> {
      return [{ uid: "demo-built-in-mic", name: "MacBook Pro Microphone", isDefault: true, isBluetooth: false }];
    },
    async openSystemAudioSettings() {},
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
          audioDeleted: false, transcriptionPercent: null, tracks: track("active"), hasRecap: false, reviewedOperationId: null,
        });
        audio.add(active.id);
        pickUp();
        active = null;
      }
      return status();
    },
    async listRecordings(eventRef: NoteReference) {
      const key = noteReferenceKey(eventRef);
      pickUp();
      return structuredClone(recordings.filter((recording) => recording.eventRef && noteReferenceKey(recording.eventRef) === key));
    },
    async deleteRecording(id: string) {
      const index = recordings.findIndex((recording) => recording.id === id);
      if (index >= 0) recordings.splice(index, 1);
      audio.delete(id);
      transcripts.delete(id);
    },
    async deleteRecordingAudio(id: string) {
      audio.delete(id);
      const recording = recordings.find((candidate) => candidate.id === id);
      if (recording) recording.audioDeleted = true;
    },
    async recordingsUsage() {
      const bytes = recordings.filter((recording) => audio.has(recording.id)).reduce((sum, recording) => sum + durationMs(recording) * AUDIO_BYTES_PER_MS, 0);
      return { recordings: recordings.length, audioBytes: Math.round(bytes) };
    },
    async deleteAllRecordingAudio() { audio = new Set(); for (const recording of recordings) recording.audioDeleted = true; },
    async deleteAllRecordings() { recordings.splice(0); audio = new Set(); transcripts.clear(); },
    async transcriptionModels() { return [modelStatus("fast"), modelStatus("accurate")]; },
    async downloadTranscriptionModel(model: TranscriptionModelName) {
      const status = modelStatus(model);
      if (status.state === "downloading") throw new Error("This model is already downloading.");
      if (status.state === "ready") throw new Error("This model is already downloaded.");
      models[model].failed = null;
      models[model].since = Date.now();
    },
    async cancelTranscriptionModelDownload(model: TranscriptionModelName) {
      const status = modelStatus(model);
      if (status.state === "downloading") { models[model].received = status.receivedBytes; models[model].since = null; }
    },
    async deleteTranscriptionModel(model: TranscriptionModelName) { models[model] = { received: 0, since: null, failed: null }; },
    async transcribeRecording(id: string) {
      const recording = recordings.find((candidate) => candidate.id === id);
      if (!recording || !audio.has(id) || recording.state === "transcribed") throw new Error("This recording can't be transcribed.");
      recording.state = "recorded";
      recording.error = null;
      pickUp();
    },
    async recordingTranscript(id: string) {
      const found = transcripts.get(id);
      if (!found) throw new Error("This recording has no transcript.");
      return structuredClone(found);
    },
    async latestTranscribedRecording() {
      pickUp();
      const found = recordings.find((recording) => recording.state === "transcribed") ?? null;
      return structuredClone(found);
    },
    async recordingReviewContext(id: string): Promise<RecordingReviewContext> {
      const recording = recordings.find((candidate) => candidate.id === id);
      if (!recording) throw new Error("Recording not found.");
      const transcript = transcripts.get(id);
      const transcriptBytes = transcript
        ? transcript.lines.reduce((sum, line) => sum + line.text.length + 24, 0)
        : 0;
      const recap = recaps.get(id) ?? null;
      return {
        recording: structuredClone(recording),
        recap: structuredClone(recap),
        transcriptBytes,
        overBudget: transcriptBytes > 400 * 1024,
        durationMs: recording.durationMs,
        hasUserNotes: Boolean(recording.eventRef),
        notesExcludedFromAi: false,
      };
    },
    async summarizeRecording(
      id: string,
      _provider: string,
      _model: string,
      _endpoint: string | null,
      _reasoning?: string,
      includeMyNotes = false,
      range: RecapTimeRange | null = null,
    ): Promise<MeetingRecap> {
      const recording = recordings.find((candidate) => candidate.id === id);
      if (!recording) throw new Error("Recording not found.");
      const transcript = transcripts.get(id);
      if (!transcript || recording.state !== "transcribed") {
        throw new Error("Transcribe this recording before summarizing it.");
      }
      const inRange = range
        ? transcript.lines.filter((line) => line.startMs >= range.startMs && line.startMs <= range.endMs)
        : transcript.lines;
      const lines = inRange.length ? inRange : transcript.lines;
      const evidenceFor = (index: number) => {
        const line = lines[index] ?? lines[0];
        if (!line) return [];
        const speakerLabel = line.speaker === "you" ? "You" : line.speaker === "others" ? "Other person" : "Speaker";
        return [{ segmentId: index, startMs: line.startMs, speaker: line.speaker, speakerLabel, text: line.text }];
      };
      const recap: MeetingRecap = {
        summary: `Reviewed status of ${recording.title}, confirmed release readiness, and aligned on shipping Thursday with the announcement Friday.`,
        decisions: lines.length
          ? [{ text: "Confirm Thursday launch and send the announcement tomorrow.", evidence: evidenceFor(lines.length - 1) }]
          : [],
        actions: lines.length >= 2
          ? [{
            text: "Finish the migration dry run today.",
            owner: transcript.speakerLabels ? "you" : "unclear",
            evidence: evidenceFor(Math.min(3, lines.length - 1)),
          }]
          : [],
        openQuestions: lines.length >= 3
          ? [{ text: "Anything still blocking the rollout after the dry run?", evidence: evidenceFor(Math.min(2, lines.length - 1)) }]
          : [],
        withheldCount: 0,
        includedNotes: includeMyNotes,
        range,
      };
      recaps.set(id, recap);
      recording.hasRecap = true;
      return structuredClone(recap);
    },
    async requestRecordingPermissions() { return { microphone: options.microphone ?? "granted" as const }; },
    async confirmRecordingQuit() { await api.stopRecording(); },
  };
  return api;
}
