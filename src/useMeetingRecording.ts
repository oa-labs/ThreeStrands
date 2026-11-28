import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { readAiProvider, readAiRequestConfig } from "./aiSettings";
import { mailClient } from "./data/client";
import type { MeetingRecording, RecordingCapabilities, RecordingSettings, RecordingStartRequest, RecordingStatus, ScheduleEvent, TranscriptionModelStatus } from "./domain";
import { errorMessage, logBackgroundFailure } from "./errors";
import { readMeetingSummaryInstructions } from "./meetingSummaryInstructions";
import { adHocRecordingTitle, RECORDING_REMINDER, stopReasonNotice } from "./recordingPolicy";
import { announceRecordingSettingsChanged, listenForRecordingEvent, listenForRecordingSettingsChanged } from "./recordingEvents";
import type { Notice } from "./useNotice";
import { useTranscriptionModels } from "./useTranscriptionModels";

export const IDLE_RECORDING_STATUS: RecordingStatus = {
  state: "idle", recordingId: null, title: null, eventRef: null, elapsedMs: 0, tracks: [],
  systemAudio: { kind: "available" }, systemAudioSilentWarning: false, lastStopReason: null, stillRecordingNudge: false,
};

export type RecordingController = {
  /** Null until the device has answered. */
  capabilities: RecordingCapabilities | null;
  /** This device can record: recording UI exists at all. */
  supported: boolean;
  /** Supported and switched on in Settings: Record, Start Recording, and the like are offered. */
  enabled: boolean;
  status: RecordingStatus;
  /** The model chosen in Settings, or null until known. */
  transcriptionModel: TranscriptionModelStatus | null;
  /** True when Apple On-Device (`SpeechAnalyzer`) is the effective transcription engine. */
  usingSpeechAnalyzer: boolean;
  /** Changes when a recording's rows may have changed, so lists read them again. */
  recordingsVersion: number;
  /** A start is waiting on the first-use consent. */
  consentPending: boolean;
  quitRequested: boolean;
  /** Non-null while the Review Follow-through modal is open for a recording. */
  reviewTarget: MeetingRecording | null;
  start(request: RecordingStartRequest): Promise<void>;
  startAdHoc(): Promise<void>;
  stop(): Promise<void>;
  pause(): Promise<void>;
  resume(): Promise<void>;
  openReview(recording: MeetingRecording): void;
  openLatestReview(): Promise<void>;
  closeReview(): void;
  acceptConsent(): Promise<void>;
  cancelConsent(): void;
  confirmQuit(): Promise<void>;
  cancelQuit(): void;
};

/** Context for a webview that cannot record: every capability is absent. */
export const UNSUPPORTED_RECORDING: RecordingController = {
  capabilities: null, supported: false, enabled: false, status: IDLE_RECORDING_STATUS, transcriptionModel: null, usingSpeechAnalyzer: false, recordingsVersion: 0,
  consentPending: false, quitRequested: false, reviewTarget: null,
  start: async () => {}, startAdHoc: async () => {}, stop: async () => {}, pause: async () => {}, resume: async () => {},
  openReview: () => {}, openLatestReview: async () => {}, closeReview: () => {},
  acceptConsent: async () => {}, cancelConsent: () => {}, confirmQuit: async () => {}, cancelQuit: () => {},
};

/**
 * Owns meeting recording in the webview: what the device can do, the recorder's
 * state (from the native events and each command's answer), the first-use
 * consent, the reminder at each start, and the quit confirmation.
 */
export function useMeetingRecording({ onNotice, onOpenSettings, onOpenMeeting }: {
  onNotice(notice: Notice): void;
  /** Opens Settings → Meeting Recording. */
  onOpenSettings(): void;
  /** Opens a meeting clicked in the menu bar that has no conferencing link. */
  onOpenMeeting?(event?: ScheduleEvent): void;
}): RecordingController {
  const [capabilities, setCapabilities] = useState<RecordingCapabilities | null>(null);
  const [settings, setSettings] = useState<RecordingSettings | null>(null);
  const [status, setStatus] = useState<RecordingStatus>(IDLE_RECORDING_STATUS);
  const [recordingsVersion, setRecordingsVersion] = useState(0);
  const [pending, setPending] = useState<RecordingStartRequest | null>(null);
  const [quitRequested, setQuitRequested] = useState(false);
  const [reviewTarget, setReviewTarget] = useState<MeetingRecording | null>(null);
  const previousState = useRef<RecordingStatus["state"]>("idle");
  const starting = useRef(false);
  const autoSummarized = useRef(new Set<string>());
  const supported = capabilities?.supported === true;

  const applyStatus = useCallback((next: RecordingStatus) => {
    const before = previousState.current;
    previousState.current = next.state;
    setStatus(next);
    if (before !== "idle" && next.state === "idle") {
      setRecordingsVersion((version) => version + 1);
      const message = stopReasonNotice(next.lastStopReason);
      if (message) onNotice({ message });
    }
  }, [onNotice]);

  const loadSettings = useCallback(() => mailClient.recordingSettings().then(setSettings), []);

  useEffect(() => {
    let active = true;
    void (async () => {
      const found = await mailClient.recordingCapabilities();
      if (!active) return;
      setCapabilities(found);
      if (!found.supported) return;
      const [current, saved] = await Promise.all([mailClient.recordingStatus(), mailClient.recordingSettings()]);
      if (!active) return;
      previousState.current = current.state;
      setStatus(current);
      setSettings(saved);
    })().catch(logBackgroundFailure("Reading recording capabilities"));
    return () => { active = false; };
  }, []);

  useEffect(() => {
    if (!supported) return;
    return listenForRecordingSettingsChanged(() => void loadSettings().catch(logBackgroundFailure("Reading recording settings")));
  }, [supported, loadSettings]);

  const begin = useCallback(async (request: RecordingStartRequest, saved: RecordingSettings) => {
    if (starting.current) return;
    starting.current = true;
    try {
      applyStatus(await mailClient.startRecording(request));
      if (saved.remindToTell) onNotice({ message: RECORDING_REMINDER });
    } catch (reason) {
      onNotice({ message: errorMessage(reason) });
    } finally {
      starting.current = false;
    }
  }, [applyStatus, onNotice]);

  const start = useCallback(async (request: RecordingStartRequest) => {
    if (previousState.current !== "idle") return;
    let saved: RecordingSettings;
    try {
      saved = await mailClient.recordingSettings();
    } catch (reason) {
      onNotice({ message: errorMessage(reason) });
      return;
    }
    setSettings(saved);
    if (!saved.recordMeetings) { onOpenSettings(); return; }
    if (!saved.consentAcknowledged) { setPending(request); return; }
    await begin(request, saved);
  }, [begin, onNotice, onOpenSettings]);

  const startAdHoc = useCallback(() => start({ eventRef: null, title: adHocRecordingTitle(new Date()) }), [start]);

  const command = useCallback(async (run: () => Promise<RecordingStatus>) => {
    try { applyStatus(await run()); } catch (reason) { onNotice({ message: errorMessage(reason) }); }
  }, [applyStatus, onNotice]);
  const stop = useCallback(() => command(() => mailClient.stopRecording()), [command]);
  const pause = useCallback(() => command(() => mailClient.pauseRecording()), [command]);
  const resume = useCallback(() => command(() => mailClient.resumeRecording()), [command]);

  const acceptConsent = useCallback(async () => {
    const request = pending;
    if (!request) return;
    try {
      const next = { ...(await mailClient.recordingSettings()), consentAcknowledged: true };
      await mailClient.setRecordingSettings(next);
      setSettings(next);
      announceRecordingSettingsChanged();
      setPending(null);
      await begin(request, next);
    } catch (reason) {
      setPending(null);
      onNotice({ message: errorMessage(reason) });
    }
  }, [begin, onNotice, pending]);
  const cancelConsent = useCallback(() => setPending(null), []);

  const confirmQuit = useCallback(async () => {
    try { await mailClient.confirmRecordingQuit(); } catch (reason) { onNotice({ message: errorMessage(reason) }); }
    setQuitRequested(false);
  }, [onNotice]);
  const cancelQuit = useCallback(() => setQuitRequested(false), []);

  const openReview = useCallback((target: MeetingRecording) => setReviewTarget(target), []);
  const closeReview = useCallback(() => setReviewTarget(null), []);
  const openLatestReview = useCallback(async () => {
    try {
      const latestRec = await mailClient.latestTranscribedRecording();
      if (!latestRec) {
        onNotice({ message: "There is no transcribed recording to review yet." });
        return;
      }
      setReviewTarget(latestRec);
    } catch (reason) {
      onNotice({ message: errorMessage(reason) });
    }
  }, [onNotice]);

  const maybeAutoSummarize = useCallback(async (recordingId: string) => {
    if (!settings?.summarizeWithAi || readAiProvider() === "none" || autoSummarized.current.has(recordingId)) return;
    try {
      const ctx = await mailClient.recordingReviewContext(recordingId);
      if (ctx.recording.state !== "transcribed" || ctx.recap || ctx.recording.reviewedOperationId || ctx.overBudget) return;
      autoSummarized.current.add(recordingId);
      const cfg = readAiRequestConfig("summarizing a meeting recording", "meetingRecap");
      const instructions = readMeetingSummaryInstructions();
      await mailClient.summarizeRecording(recordingId, cfg.provider, cfg.model, cfg.endpoint, cfg.reasoning, false, null, instructions || null);
    } catch (reason) {
      logBackgroundFailure("Summarizing meeting recording")(reason);
    }
  }, [settings?.summarizeWithAi]);

  // The listeners call through a ref so they are set up once, not on each render.
  const latest = useRef({ applyStatus, start, startAdHoc, onOpenSettings, onOpenMeeting, openLatestReview, maybeAutoSummarize, onNotice });
  latest.current = { applyStatus, start, startAdHoc, onOpenSettings, onOpenMeeting, openLatestReview, maybeAutoSummarize, onNotice };
  useEffect(() => {
    if (!supported) return;
    const stops = [
      listenForRecordingEvent("recording-state", (next) => latest.current.applyStatus(next)),
      listenForRecordingEvent("recordings-changed", ({ recordingId }) => {
        setRecordingsVersion((version) => version + 1);
        void latest.current.maybeAutoSummarize(recordingId);
      }),
      listenForRecordingEvent("recording-tray-command", (payload) => {
        if (payload.command === "start") void latest.current.startAdHoc();
        else if (payload.command === "startOffer") {
          if (payload.startRequest) void latest.current.start(payload.startRequest);
          else void latest.current.startAdHoc();
        }
        else if (payload.command === "openSettings") latest.current.onOpenSettings();
        else if (payload.command === "openMeeting") latest.current.onOpenMeeting?.(payload.meetingEvent);
        else if (payload.command === "review") {
          if (payload.recordingId) {
            void mailClient.recordingReviewContext(payload.recordingId)
              .then((ctx) => setReviewTarget(ctx.recording), (reason) => latest.current.onNotice({ message: errorMessage(reason) }));
          } else {
            void latest.current.openLatestReview();
          }
        }
      }),
      listenForRecordingEvent("recording-quit-requested", () => setQuitRequested(true)),
    ];
    return () => { for (const stopListening of stops) stopListening(); };
  }, [supported]);

  const enabled = supported && settings?.recordMeetings === true;
  const usingSpeechAnalyzer = capabilities?.speechAnalyzer === true && settings?.transcriptionEngine !== "whisper";
  const models = useTranscriptionModels(supported);
  const transcriptionModel = models?.find((model) => model.model === (settings?.transcriptionModel ?? "fast")) ?? null;
  return useMemo(() => ({
    capabilities, supported, enabled, status, transcriptionModel, usingSpeechAnalyzer, recordingsVersion,
    consentPending: pending !== null, quitRequested, reviewTarget,
    start, startAdHoc, stop, pause, resume, openReview, openLatestReview, closeReview, acceptConsent, cancelConsent, confirmQuit, cancelQuit,
  }), [capabilities, supported, enabled, status, transcriptionModel, usingSpeechAnalyzer, recordingsVersion, pending, quitRequested, reviewTarget, start, startAdHoc, stop, pause, resume, openReview, openLatestReview, closeReview, acceptConsent, cancelConsent, confirmQuit, cancelQuit]);
}
