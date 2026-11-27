import { afterEach, describe, expect, it, vi } from "vitest";
import type { NoteReference } from "../noteEntities";
import { createDemoRecordings } from "./demoRecordings";

const meeting: NoteReference = { version: 1, kind: "event", id: "evt", account: "me@example.com", calendar: "work" };

describe("demo transcription", () => {
  afterEach(() => vi.useRealTimers());

  it("progresses a download from the user's Download, can cancel and resume it, and deletes", async () => {
    vi.useFakeTimers();
    const demo = createDemoRecordings();
    expect((await demo.transcriptionModels()).map((model) => model.state)).toEqual(["absent", "absent"]);
    await demo.downloadTranscriptionModel("fast");
    vi.advanceTimersByTime(3000);
    const [halfway] = await demo.transcriptionModels();
    expect(halfway.state).toBe("downloading");
    expect(halfway.receivedBytes).toBeGreaterThan(0);
    await expect(demo.downloadTranscriptionModel("fast")).rejects.toThrow("already downloading");
    await demo.cancelTranscriptionModelDownload("fast");
    const [cancelled] = await demo.transcriptionModels();
    expect(cancelled.state).toBe("absent");
    expect(cancelled.receivedBytes).toBeGreaterThan(0);
    await demo.downloadTranscriptionModel("fast");
    vi.advanceTimersByTime(6000);
    expect((await demo.transcriptionModels())[0]).toMatchObject({ state: "ready", receivedBytes: 190_000_000 });
    await demo.deleteTranscriptionModel("fast");
    expect((await demo.transcriptionModels())[0]).toMatchObject({ state: "absent", receivedBytes: 0 });
  });

  it("waits for a model, then transcribes a recording with You and Others lines", async () => {
    vi.useFakeTimers();
    const demo = createDemoRecordings();
    await demo.startRecording({ eventRef: meeting, title: "Weekly Sync" });
    vi.advanceTimersByTime(60_000);
    await demo.stopRecording();
    const [waiting] = await demo.listRecordings(meeting);
    expect(waiting).toMatchObject({ state: "recorded", audioDeleted: false });
    await expect(demo.recordingTranscript(waiting.id)).rejects.toThrow();
    await demo.downloadTranscriptionModel("fast");
    vi.advanceTimersByTime(6000);
    const [done] = await demo.listRecordings(meeting);
    expect(done.state).toBe("transcribed");
    const transcript = await demo.recordingTranscript(done.id);
    expect(transcript.speakerLabels).toBe(true);
    expect(new Set(transcript.lines.map((line) => line.speaker))).toEqual(new Set(["you", "others"]));
  });

  it("seeds a transcribed recording and a microphone-only one with no speaker labels", async () => {
    const demo = createDemoRecordings({ seed: [meeting] });
    const rows = await demo.listRecordings(meeting);
    expect(rows.map((row) => row.state)).toEqual(["transcribed", "transcribed"]);
    const labels = await Promise.all(rows.map(async (row) => (await demo.recordingTranscript(row.id)).speakerLabels));
    expect(labels.sort()).toEqual([false, true]);
    await demo.deleteRecording(rows[0].id);
    await expect(demo.recordingTranscript(rows[0].id)).rejects.toThrow();
  });
});
