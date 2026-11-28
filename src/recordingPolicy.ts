import type { AiProvider } from "./aiSettings";
import type { MeetingRecording, RecordingStatus, RecordingStopReason, RecordingTranscript, ScheduleEvent, TranscriptionModelStatus } from "./domain";
import { formatClockTime, isDeclined } from "./agendaEvents";
import { meetingEventReference } from "./meetingNoteEntry";
import { noteReferenceKey, type NoteReference } from "./noteEntities";
import { isWorkingLocation } from "./WorkingLocation";

/** How long before a meeting starts that Record is offered for it. Matches the tray menu's "starts within 10 min". */
export const RECORD_LEAD_MS = 10 * 60_000;

export const RECORDING_REMINDER = "Let everyone know this meeting is being recorded.";

/**
 * Whether Record is offered for an event at `now`: a timed, undeclined meeting
 * a note can link, that is under way or starts within the lead window. An
 * ended meeting, an all-day event, and one further out are not.
 */
export function canRecordEvent(event: ScheduleEvent, now: Date): boolean {
  if (event.allDay || isDeclined(event) || isWorkingLocation(event) || !meetingEventReference(event)) return false;
  const start = new Date(event.start).getTime();
  const end = new Date(event.end).getTime();
  if (Number.isNaN(start) || Number.isNaN(end)) return false;
  const at = now.getTime();
  return at < end && at >= start - RECORD_LEAD_MS;
}

/**
 * What Record offers for an event: Stop Recording while this occurrence is the
 * one being recorded, Record when it is close enough and nothing else is being
 * recorded, otherwise nothing.
 */
export function recordOffer(status: RecordingStatus, event: ScheduleEvent, now: Date): "record" | "stop" | null {
  const reference = meetingEventReference(event);
  if (!reference) return null;
  if (isRecordingOf(status, reference)) return "stop";
  return status.state === "idle" && canRecordEvent(event, now) ? "record" : null;
}

/** A recording is of this occurrence when its event reference names it. */
export function isRecordingOf(status: RecordingStatus, reference: NoteReference | null): boolean {
  return status.state !== "idle" && Boolean(reference) && Boolean(status.eventRef) && noteReferenceKey(status.eventRef!) === noteReferenceKey(reference!);
}

/** The title an ad-hoc recording gets, such as "Meeting at 3:04 PM". */
export function adHocRecordingTitle(now: Date): string {
  return `Meeting at ${formatClockTime(now)}`;
}

/** `m:ss`, or `h:mm:ss` from an hour on. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const seconds = String(total % 60).padStart(2, "0");
  const minutes = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}` : `${minutes}:${seconds}`;
}

const minutesLabel = (ms: number) => `${Math.max(1, Math.round(ms / 60_000))} min`;

/**
 * A failed recording with no audio on disk: either nothing was captured, or
 * the audio is gone, so there is nothing to transcribe again. With audio, the
 * failure was its transcription's. Track states can't tell these apart: every
 * track can fail after audio was saved.
 */
const failedWithoutAudio = (recording: MeetingRecording) => recording.audioDeleted;

/**
 * The detail line under a Recording row. `model` is the chosen transcription
 * model, null until known; `percent` is the latest live progress, which beats
 * the percent the list carried.
 */
export function recordingDetail(
  recording: MeetingRecording,
  model: TranscriptionModelStatus | null = null,
  percent: number | null = null,
  usingSpeechAnalyzer = false,
): string {
  const minutes = minutesLabel(recording.durationMs);
  const parts: string[] = [];
  switch (recording.state) {
    case "recording": parts.push("Recording…"); break;
    case "interrupted": parts.push("Interrupted", `${minutes} saved`); break;
    case "failed": parts.push(failedWithoutAudio(recording) ? "Failed" : "Transcription failed"); break;
    case "transcribing": {
      const shown = percent ?? recording.transcriptionPercent;
      parts.push(shown === null ? "Transcribing…" : `Transcribing… ${shown}%`);
      break;
    }
    case "transcribed": parts.push(minutes, "Transcribed"); break;
    case "recorded":
      parts.push(minutes, !usingSpeechAnalyzer && model && model.state !== "ready" ? "Waiting for a transcription model" : "Waiting to transcribe");
      break;
  }
  const system = recording.tracks.find((track) => track.track === "system");
  if (recording.state !== "recording" && recording.state !== "failed" && system?.state !== "active") parts.push("Microphone only");
  return parts.join(" · ");
}

/** Show Transcript is offered once there is a transcript. */
export const canShowTranscript = (recording: MeetingRecording) => recording.state === "transcribed";

/** Retry Transcription is offered for a failed recording whose audio still exists. */
export const canRetryTranscription = (recording: MeetingRecording) => recording.state === "failed" && !failedWithoutAudio(recording);

/** `[12:34] You: text`, or `[1:02:03] text` without labels; one line per transcript line. */
export function transcriptCopyText(transcript: RecordingTranscript): string {
  return transcript.lines
    .map((line) => `[${formatElapsed(line.startMs)}] ${transcript.speakerLabels && line.speaker ? `${speakerLabel(line.speaker, line.speakerLabel)}: ` : ""}${line.text}`)
    .join("\n");
}

export const speakerLabel = (speaker: "you" | "others", remoteLabel?: string | null) => {
  if (speaker === "you") return "You";
  const trimmed = remoteLabel?.trim();
  return trimmed ? `Others (${trimmed})` : "Others";
};

export const isCloudTranscriptionProvider = (provider: AiProvider): boolean =>
  provider === "openrouter" || provider === "openai" || provider === "fireworks" || provider === "custom";

export const CUSTOM_VOCAB_MAX_TERMS = 100;
export const CUSTOM_VOCAB_MAX_TERM_CHARS = 64;

export interface CustomVocabMergeResult {
  next: string[];
  added: number;
  duplicates: number;
  tooLong: number;
  truncated: number;
}

const OUTER_VOCAB_NOISE = /^[^\p{L}\p{N}\-'.+/]+|[^\p{L}\p{N}\-'.+/]+$/gu;

function cleanCustomVocabTerm(raw: string): string {
  const collapsed = raw.replace(/[\u0000-\u001F\u007F]/g, " ").replace(/\s+/g, " ").trim();
  return collapsed.replace(OUTER_VOCAB_NOISE, "").trim();
}

/** Splits comma- or newline-separated vocabulary input into raw term candidates. */
export function parseCustomVocabInput(raw: string): string[] {
  return raw.split(/[,\n\r]+/);
}

/** Sanitizes, deduplicates case-insensitively, and caps custom vocabulary terms. */
export function mergeCustomVocab(existing: string[], incoming: string[]): CustomVocabMergeResult {
  const seen = new Set(existing.map((term) => term.toLowerCase()));
  const next = [...existing];
  let added = 0;
  let duplicates = 0;
  let tooLong = 0;
  let truncated = 0;

  for (const raw of incoming) {
    const cleaned = cleanCustomVocabTerm(raw);
    if (!cleaned) continue;
    if ([...cleaned].length > CUSTOM_VOCAB_MAX_TERM_CHARS) {
      tooLong += 1;
      continue;
    }
    const key = cleaned.toLowerCase();
    if (seen.has(key)) {
      duplicates += 1;
      continue;
    }
    if (next.length >= CUSTOM_VOCAB_MAX_TERMS) {
      truncated += 1;
      continue;
    }
    seen.add(key);
    next.push(cleaned);
    added += 1;
  }

  return { next, added, duplicates, tooLong, truncated };
}

export function defaultCloudTranscriptionModel(provider: AiProvider): string {
  switch (provider) {
    case "openrouter": return "microsoft/mai-transcribe-2";
    case "openai": return "gpt-4o-mini-transcribe";
    case "fireworks": return "whisper-v3-turbo";
    default: return "";
  }
}

/** The notice for a recording the app ended on its own, or null for a normal stop. */
export function stopReasonNotice(reason: RecordingStopReason | null): string | null {
  switch (reason) {
    case "lengthLimit": return "Recording stopped because it reached the length limit. The audio was saved.";
    case "lowDisk": return "Recording stopped because this Mac is low on disk space. The audio was saved.";
    // Earlier audio may have been saved, so this says only why it stopped.
    case "allTracksFailed": return "Recording stopped because the microphone and computer audio stopped working.";
    case "systemSleep": return "Recording stopped because this Mac went to sleep. The audio was saved.";
    default: return null;
  }
}
