import { describe, expect, it } from "vitest";
import type { MeetingRecording, RecordingStatus, ScheduleEvent } from "./domain";
import { meetingEventReference } from "./meetingNoteEntry";
import { adHocRecordingTitle, canRecordEvent, formatElapsed, isRecordingOf, RECORD_LEAD_MS, recordingDetail, recordOffer, stopReasonNotice } from "./recordingPolicy";

const event = (overrides: Partial<ScheduleEvent> = {}): ScheduleEvent => ({
  id: "evt", accountId: "me@example.com", calendarId: "work", title: "Weekly Sync",
  start: "2026-11-27T15:00:00Z", end: "2026-11-27T16:00:00Z", allDay: false, ...overrides,
});
const at = (iso: string) => new Date(iso);
const status = (overrides: Partial<RecordingStatus> = {}): RecordingStatus => ({
  state: "idle", recordingId: null, title: null, eventRef: null, elapsedMs: 0, tracks: [],
  systemAudio: { kind: "available" }, lastStopReason: null, ...overrides,
});
const recording = (overrides: Partial<MeetingRecording> = {}): MeetingRecording => ({
  id: "r1", eventRef: null, title: "Weekly Sync", startedAt: "2026-11-27T15:00:00Z", endedAt: "2026-11-27T15:42:00Z",
  durationMs: 42 * 60_000, state: "recorded", error: null,
  tracks: [
    { track: "mic", state: "active", deviceName: "Mic", firstFrameOffsetMs: 0, error: null },
    { track: "system", state: "active", deviceName: null, firstFrameOffsetMs: 0, error: null },
  ],
  ...overrides,
});

describe("canRecordEvent", () => {
  it("offers Record from 10 minutes before the start", () => {
    expect(RECORD_LEAD_MS).toBe(10 * 60_000);
    expect(canRecordEvent(event(), at("2026-11-27T14:49:59Z"))).toBe(false);
    expect(canRecordEvent(event(), at("2026-11-27T14:50:00Z"))).toBe(true);
    expect(canRecordEvent(event(), at("2026-11-27T14:55:00Z"))).toBe(true);
  });

  it("offers Record while the meeting is under way, and not once it has ended", () => {
    expect(canRecordEvent(event(), at("2026-11-27T15:00:00Z"))).toBe(true);
    expect(canRecordEvent(event(), at("2026-11-27T15:59:59Z"))).toBe(true);
    expect(canRecordEvent(event(), at("2026-11-27T16:00:00Z"))).toBe(false);
    expect(canRecordEvent(event(), at("2026-11-27T17:00:00Z"))).toBe(false);
  });

  it("is for timed meetings a note can link, that you have not declined", () => {
    const now = at("2026-11-27T15:10:00Z");
    expect(canRecordEvent(event({ allDay: true, start: "2026-11-27", end: "2026-11-28" }), now)).toBe(false);
    expect(canRecordEvent(event({ responseStatus: "declined" }), now)).toBe(false);
    expect(canRecordEvent(event({ calendarId: undefined }), now)).toBe(false);
    expect(canRecordEvent(event({ eventType: "workingLocation" }), now)).toBe(false);
  });
});

describe("recordOffer", () => {
  const now = at("2026-11-27T15:10:00Z");
  const reference = meetingEventReference(event())!;

  it("offers Record when idle, and Stop for the occurrence being recorded", () => {
    expect(recordOffer(status(), event(), now)).toBe("record");
    expect(recordOffer(status({ state: "recording", eventRef: reference }), event(), now)).toBe("stop");
    expect(recordOffer(status({ state: "paused", eventRef: reference }), event(), now)).toBe("stop");
  });

  it("keeps Stop for the recorded occurrence after its meeting ends, and offers nothing for another recording", () => {
    const later = at("2026-11-27T17:00:00Z");
    expect(recordOffer(status({ state: "recording", eventRef: reference }), event(), later)).toBe("stop");
    expect(recordOffer(status({ state: "recording", eventRef: null }), event(), now)).toBeNull();
    expect(recordOffer(status({ state: "recording", eventRef: { ...reference, id: "other" } }), event(), now)).toBeNull();
  });
});

describe("isRecordingOf", () => {
  it("compares references by content and ignores an idle recorder", () => {
    const reference = meetingEventReference(event())!;
    expect(isRecordingOf(status({ state: "recording", eventRef: { ...reference } }), reference)).toBe(true);
    expect(isRecordingOf(status({ eventRef: reference }), reference)).toBe(false);
    expect(isRecordingOf(status({ state: "recording" }), reference)).toBe(false);
  });
});

describe("recordingDetail", () => {
  it("describes each state", () => {
    expect(recordingDetail(recording())).toBe("42 min · Recorded");
    expect(recordingDetail(recording({ state: "recording", durationMs: 0 }))).toBe("Recording…");
    expect(recordingDetail(recording({ state: "interrupted", durationMs: 18 * 60_000 }))).toBe("Interrupted · 18 min saved");
    expect(recordingDetail(recording({ state: "failed" }))).toBe("Failed");
  });

  it("says when only the microphone was recorded", () => {
    const micOnly = recording({ tracks: [
      { track: "mic", state: "active", deviceName: "Mic", firstFrameOffsetMs: 0, error: null },
      { track: "system", state: "unavailable", deviceName: null, firstFrameOffsetMs: null, error: null },
    ] });
    expect(recordingDetail(micOnly)).toBe("42 min · Recorded · Microphone only");
    expect(recordingDetail({ ...micOnly, tracks: micOnly.tracks.slice(0, 1) })).toBe("42 min · Recorded · Microphone only");
    expect(recordingDetail({ ...micOnly, state: "interrupted", durationMs: 18 * 60_000 })).toBe("Interrupted · 18 min saved · Microphone only");
  });

  it("never shows less than a minute as zero", () => {
    expect(recordingDetail(recording({ durationMs: 10_000 }))).toBe("1 min · Recorded");
  });
});

describe("formatting", () => {
  it("formats elapsed time as m:ss and h:mm:ss", () => {
    expect(formatElapsed(0)).toBe("0:00");
    expect(formatElapsed(59_999)).toBe("0:59");
    expect(formatElapsed(61_000)).toBe("1:01");
    expect(formatElapsed(3_599_000)).toBe("59:59");
    expect(formatElapsed(3_600_000)).toBe("1:00:00");
    expect(formatElapsed(-5)).toBe("0:00");
  });

  it("titles an ad-hoc recording by the time it started", () => {
    expect(adHocRecordingTitle(new Date(2026, 10, 27, 15, 4))).toMatch(/^Meeting at 3:04\s?pm$/i);
  });

  it("explains only recordings the app ended itself", () => {
    expect(stopReasonNotice(null)).toBeNull();
    expect(stopReasonNotice("lengthLimit")).toContain("length limit");
    expect(stopReasonNotice("lowDisk")).toContain("disk space");
    expect(stopReasonNotice("allTracksFailed")).toContain("stopped working");
  });
});
