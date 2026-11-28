import { useCallback, useEffect, useId, useRef, useState } from "react";
import { readAiEndpoint, readAiProvider } from "./aiSettings";
import { mailClient } from "./data/client";
import type { AudioInputDevice, KeepAudio, RecordingCapabilities, RecordingSettings, RecordingsUsage, TranscriptionModelName, TranscriptionModelStatus, TrayNextMeetingTitle, VocabularySuggestion } from "./domain";
import { errorMessage } from "./errors";
import { InlineConfirm } from "./InlineConfirm";
import {
  MAX_MEETING_SUMMARY_INSTRUCTIONS_CHARS,
  MEETING_SUMMARY_PRESETS,
  saveMeetingSummaryInstructions,
  useMeetingSummaryInstructions,
} from "./meetingSummaryInstructions";
import {
  CUSTOM_VOCAB_MAX_TERM_CHARS,
  CUSTOM_VOCAB_MAX_TERMS,
  defaultCloudTranscriptionModel,
  isCloudTranscriptionProvider,
  mergeCustomVocab,
  parseCustomVocabInput,
  type CustomVocabMergeResult,
} from "./recordingPolicy";
import { announceRecordingSettingsChanged } from "./recordingEvents";
import { formatAttachmentSize } from "./threadPresentation";
import { useTranscriptionModels } from "./useTranscriptionModels";

const KEEP_AUDIO: { value: KeepAudio; label: string }[] = [
  { value: "days7", label: "7 Days" },
  { value: "days30", label: "30 Days" },
  { value: "untilDeleted", label: "Until Deleted" },
  { value: "untilTranscribed", label: "Until Transcribed" },
];

const TRAY_COUNTDOWN_OPTIONS: { value: TrayNextMeetingTitle; label: string }[] = [
  { value: "titleAndTime", label: "Title & time" },
  { value: "timeOnly", label: "Time only" },
  { value: "off", label: "Icon only" },
];

const MODELS: { value: TranscriptionModelName; label: string; note: string }[] = [
  { value: "fast", label: "Whisper Fast", note: "English only" },
  { value: "accurate", label: "Whisper Accurate", note: "Many languages" },
];

const LANGUAGES: { value: string; label: string }[] = [
  { value: "", label: "Auto" },
  { value: "en", label: "English" },
  { value: "es", label: "Spanish" },
  { value: "fr", label: "French" },
  { value: "de", label: "German" },
  { value: "it", label: "Italian" },
  { value: "pt", label: "Portuguese" },
  { value: "nl", label: "Dutch" },
  { value: "pl", label: "Polish" },
  { value: "sv", label: "Swedish" },
  { value: "ja", label: "Japanese" },
  { value: "ko", label: "Korean" },
  { value: "zh", label: "Chinese" },
  { value: "hi", label: "Hindi" },
];

/** Decimal megabytes, the way the model's download size is quoted. */
const modelSize = (bytes: number) => `${Math.round(bytes / 1_000_000)} MB`;
const modelPercent = (model: TranscriptionModelStatus) => (model.bytes > 0 ? Math.min(100, Math.floor((model.receivedBytes / model.bytes) * 100)) : 0);

const MICROPHONE_DENIED = "Microphone access is off for ThreeStrands. Turn it on in System Settings → Privacy & Security → Microphone, then try again.";
const MICROPHONE_RESTRICTED = "Microphone access is restricted on this Mac, so ThreeStrands can't record. Ask whoever manages this Mac to allow it in System Settings → Privacy & Security → Microphone.";

type Confirming = "audio" | "recordings" | null;

function useRecordingSettingsState() {
  const [settings, setSettings] = useState<RecordingSettings | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void mailClient.recordingSettings().then(
      (saved) => { if (active) setSettings(saved); },
      (reason: unknown) => { if (active) setError(errorMessage(reason)); },
    );
    return () => { active = false; };
  }, []);

  const save = useCallback(async (next: RecordingSettings) => {
    await mailClient.setRecordingSettings(next);
    setSettings(next);
    announceRecordingSettingsChanged();
  }, []);

  const change = useCallback(async (update: Partial<RecordingSettings>) => {
    if (!settings) return;
    setError(null);
    try { await save({ ...settings, ...update }); } catch (reason) { setError(errorMessage(reason)); }
  }, [settings, save]);

  return { settings, error, setError, save, change };
}

type ModelStatusTone = "neutral" | "attention" | "ready" | "error";

function whisperModelStatus(model: TranscriptionModelStatus | undefined, selected: boolean): { text: string; tone: ModelStatusTone } {
  if (!model) return { text: "Checking…", tone: "neutral" };
  const percent = modelPercent(model);
  const partial = model.receivedBytes > 0 && model.state !== "ready";
  switch (model.state) {
    case "absent":
      if (partial) return { text: `Partly downloaded, ${percent}%`, tone: selected ? "attention" : "neutral" };
      return { text: selected ? "Needs download" : "Not downloaded", tone: selected ? "attention" : "neutral" };
    case "downloading":
      return { text: `Downloading… ${percent}%`, tone: "neutral" };
    case "verifying":
      return { text: "Checking the download…", tone: "neutral" };
    case "ready":
      return { text: "Downloaded", tone: "ready" };
    case "failed":
      return { text: "Download failed", tone: "error" };
  }
}

function formatVocabFeedback(result: CustomVocabMergeResult): string | null {
  if (result.added > 0) {
    const base = `Added ${result.added} ${result.added === 1 ? "term" : "terms"}`;
    const skipped: string[] = [];
    if (result.duplicates > 0) skipped.push(`${result.duplicates} ${result.duplicates === 1 ? "duplicate" : "duplicates"}`);
    if (result.tooLong > 0) skipped.push(`${result.tooLong} over ${CUSTOM_VOCAB_MAX_TERM_CHARS} characters`);
    if (result.truncated > 0) skipped.push(`${result.truncated} over the ${CUSTOM_VOCAB_MAX_TERMS}-term limit`);
    return skipped.length > 0 ? `${base}; skipped ${skipped.join(", ")}.` : `${base}.`;
  }
  if (result.truncated > 0) return `Vocabulary limit of ${CUSTOM_VOCAB_MAX_TERMS} terms reached.`;
  if (result.tooLong > 0) return `Terms must be ${CUSTOM_VOCAB_MAX_TERM_CHARS} characters or fewer.`;
  if (result.duplicates > 0) {
    return result.duplicates === 1 ? "Already in your vocabulary." : "Those terms are already in your vocabulary.";
  }
  return null;
}

/** One selectable transcription engine/model row with inline status and download controls. */
function TranscriptionModelRow({
  radioGroupName,
  value,
  label,
  meta,
  status,
  statusTone,
  selected,
  cloudIsDefault,
  selectDisabled,
  onSelect,
  model,
  onDownload,
  onCancel,
  onDelete,
  busy,
}: {
  radioGroupName: string;
  value: "apple" | TranscriptionModelName;
  label: string;
  meta: string;
  status: string;
  statusTone: ModelStatusTone;
  selected: boolean;
  cloudIsDefault: boolean;
  selectDisabled: boolean;
  onSelect(): void;
  model?: TranscriptionModelStatus;
  onDownload?(): void;
  onCancel?(): void;
  onDelete?(): void;
  busy?: boolean;
}) {
  const nameId = useId();
  const metaId = useId();
  const percent = model ? modelPercent(model) : 0;

  return (
    <div
      className={`transcription-model${selected ? " is-selected" : ""}`}
      role="group"
      aria-label={`${label} model`}
      aria-describedby={metaId}
    >
      <div className="transcription-model-row">
        <label className="transcription-model-choice">
          <input
            type="radio"
            name={radioGroupName}
            value={value}
            checked={selected}
            disabled={selectDisabled}
            aria-labelledby={nameId}
            aria-describedby={metaId}
            onChange={onSelect}
          />
          <span className="transcription-model-text">
            <span className="transcription-model-title">
              <span className="transcription-model-name" id={nameId}>{label}</span>
              {selected ? (
                <span className={`transcription-model-badge${cloudIsDefault ? " is-fallback" : ""}`}>
                  {cloudIsDefault ? "Fallback" : "Active"}
                </span>
              ) : null}
            </span>
            <span className="transcription-model-meta" id={metaId}>
              <span className="settings-hint">{meta}</span>
              <span className="transcription-model-sep" aria-hidden="true">·</span>
              <span className={`settings-hint transcription-model-status is-${statusTone}`} role="status">{status}</span>
            </span>
          </span>
        </label>
        <div className="transcription-model-actions">
          {model?.state === "absent" && onDownload ? (
            <button type="button" className={selected ? "btn btn-primary btn-sm" : "btn btn-sm"} disabled={busy} onClick={onDownload}>Download</button>
          ) : null}
          {model?.state === "failed" && onDownload ? (
            <button type="button" className="btn btn-sm" disabled={busy} onClick={onDownload}>Try Again</button>
          ) : null}
          {model?.state === "downloading" && onCancel ? (
            <button type="button" className="btn btn-sm" disabled={busy} onClick={onCancel}>Cancel</button>
          ) : null}
          {model?.state === "ready" && onDelete ? (
            <button type="button" className="btn btn-sm" disabled={busy} onClick={onDelete}>Delete Model</button>
          ) : null}
        </div>
      </div>
      {model?.state === "downloading" || model?.state === "verifying" ? (
        <progress className="transcription-model-progress" max={100} value={model.state === "downloading" ? percent : undefined} aria-label={`${label} model download`} />
      ) : null}
      {model?.state === "failed" && model.error ? <p className="form-error" role="alert">{model.error}</p> : null}
    </div>
  );
}

/** Settings → Meeting Recording → Recording & Menu Bar. Everything saves as it changes. */
export function MeetingRecordingSettings({ capabilities }: { capabilities: RecordingCapabilities }) {
  const { settings, error, setError, save, change } = useRecordingSettingsState();
  const [usage, setUsage] = useState<RecordingsUsage | null>(null);
  const [devices, setDevices] = useState<AudioInputDevice[]>([]);
  const [busy, setBusy] = useState(false);
  const [permissionNote, setPermissionNote] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<Confirming>(null);
  const [message, setMessage] = useState<string | null>(null);
  const ids = {
    record: useId(), microphone: useId(), keep: useId(), offer: useId(),
    upcomingTray: useId(), trayTitle: useId(), remind: useId(), usage: useId(),
  };

  const loadUsage = useCallback(async () => setUsage(await mailClient.recordingsUsage()), []);
  useEffect(() => {
    let active = true;
    void Promise.all([mailClient.recordingsUsage(), mailClient.recordingInputDevices()]).then(
      ([used, inputs]) => {
        if (!active) return;
        setUsage(used);
        setDevices(inputs);
      },
      (reason: unknown) => { if (active) setError(errorMessage(reason)); },
    );
    return () => { active = false; };
  }, [setError]);

  const toggleRecording = async (on: boolean) => {
    if (!settings) return;
    setError(null);
    setPermissionNote(null);
    if (!on) { await change({ recordMeetings: false }); return; }
    setBusy(true);
    try {
      const { microphone } = await mailClient.requestRecordingPermissions();
      if (microphone === "granted") await save({ ...settings, recordMeetings: true });
      else setPermissionNote(microphone === "restricted" ? MICROPHONE_RESTRICTED : MICROPHONE_DENIED);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const deleteAll = async (kind: Exclude<Confirming, null>) => {
    setBusy(true);
    setError(null);
    try {
      if (kind === "audio") await mailClient.deleteAllRecordingAudio();
      else await mailClient.deleteAllRecordings();
      await loadUsage();
      setMessage(kind === "audio" ? "Deleted all recording audio from this Mac." : "Deleted all recordings from this Mac.");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
      setConfirming(null);
    }
  };

  const confirm = (kind: Confirming) => { setMessage(null); setConfirming(kind); };
  const system = capabilities.systemAudio;
  const activeDevice = settings?.microphoneDeviceUid
    ? devices.find((device) => device.uid === settings.microphoneDeviceUid)
    : devices.find((device) => device.isDefault);

  return (
    <section className="settings-section wide-label-settings" aria-label="Recording & Menu Bar">
      <h3>Capture & Audio</h3>
      {system.kind === "needsMacos" ? (
        <p className="settings-hint" role="note">
          System audio needs macOS 14.4 or later; this Mac has macOS {system.major}.{system.minor}. Recordings will have the microphone only.
        </p>
      ) : null}
      <label className="settings-switch">
        <span>Record Meetings</span>
        <input type="checkbox" checked={settings?.recordMeetings ?? false} disabled={!settings || busy} aria-describedby={ids.record}
          onChange={(event) => void toggleRecording(event.target.checked)} />
      </label>
      <p className="settings-hint" id={ids.record}>
        Records your microphone and your Mac's audio from the menu bar, a meeting's details, or Today. Turning this on asks for microphone access. Audio stays on this Mac.
      </p>
      {permissionNote ? <p className="form-error" role="alert">{permissionNote}</p> : null}

      <label className="settings-field settings-field-row">
        <span>Microphone</span>
        <select value={settings?.microphoneDeviceUid ?? ""} disabled={!settings} aria-describedby={ids.microphone}
          onChange={(event) => void change({ microphoneDeviceUid: event.target.value || null })}>
          <option value="">System Default</option>
          {devices.map((device) => (
            <option key={device.uid} value={device.uid}>
              {device.isDefault ? `${device.name} (Default)` : device.name}
            </option>
          ))}
          {settings?.microphoneDeviceUid && !devices.some((device) => device.uid === settings.microphoneDeviceUid) ? (
            <option value={settings.microphoneDeviceUid}>{settings.microphoneDeviceUid} (Unavailable)</option>
          ) : null}
        </select>
      </label>
      <p className="settings-hint settings-field-detail" id={ids.microphone}>
        Choose a specific microphone to avoid switching audio devices mid-call, or follow your Mac's default input.
      </p>
      {activeDevice?.isBluetooth ? (
        <p className="settings-hint settings-field-detail" role="note">
          Bluetooth headsets can drop audio quality into headset mode when recorded as a microphone; using your Mac's built-in mic keeps speaker audio crisp.
        </p>
      ) : null}

      <label className="settings-field settings-field-row">
        <span>Keep Audio</span>
        <select value={settings?.keepAudio ?? "days7"} disabled={!settings} aria-describedby={ids.keep}
          onChange={(event) => void change({ keepAudio: event.target.value as KeepAudio })}>
          {KEEP_AUDIO.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </select>
      </label>
      <p className="settings-hint settings-field-detail" id={ids.keep}>
        How long a recording's audio stays on this Mac before ThreeStrands deletes it. Until Transcribed deletes it once its transcript exists.
      </p>

      <h3>Menu Bar & Reminders</h3>
      <label className="settings-switch">
        <span>Show Upcoming Meetings in Menu Bar</span>
        <input type="checkbox" checked={settings?.showUpcomingMeetingsInTray ?? true} disabled={!settings} aria-describedby={ids.upcomingTray}
          onChange={(event) => void change({ showUpcomingMeetingsInTray: event.target.checked })} />
      </label>
      <p className="settings-hint" id={ids.upcomingTray}>
        Lists today&apos;s remaining meetings (or tomorrow&apos;s first when today is free) in the menu bar so you can click to join in one click, and keeps the menu bar icon visible while meetings remain.
      </p>

      <label className="settings-field settings-field-row">
        <span>Menu Bar Countdown</span>
        <select value={settings?.trayNextMeetingTitle ?? "titleAndTime"} disabled={!settings || !(settings.showUpcomingMeetingsInTray)} aria-describedby={ids.trayTitle}
          onChange={(event) => void change({ trayNextMeetingTitle: event.target.value as TrayNextMeetingTitle })}>
          {TRAY_COUNTDOWN_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </select>
      </label>
      <p className="settings-hint settings-field-detail" id={ids.trayTitle}>
        Shows the next meeting beside the menu bar icon while it is live or starting within two hours.
      </p>

      <label className="settings-switch">
        <span>Offer to Record Meetings</span>
        <input type="checkbox" checked={settings?.offerToRecord ?? true} disabled={!settings} aria-describedby={ids.offer}
          onChange={(event) => void change({ offerToRecord: event.target.checked })} />
      </label>
      <p className="settings-hint" id={ids.offer}>
        Offers the current or upcoming meeting in the menu bar and shows a badge when a conferencing app or meeting in a browser uses your microphone. Never starts or stops a recording on its own.
      </p>

      <label className="settings-switch">
        <span>Remind Me to Tell Participants</span>
        <input type="checkbox" checked={settings?.remindToTell ?? true} disabled={!settings} aria-describedby={ids.remind}
          onChange={(event) => void change({ remindToTell: event.target.checked })} />
      </label>
      <p className="settings-hint" id={ids.remind}>
        Shows a notice each time a recording starts. ThreeStrands never posts to a meeting's chat or invitation for you.
      </p>

      <h3>Recordings on This Mac</h3>
      <p className="settings-hint" id={ids.usage} role="status">
        {usage === null ? "Checking…"
          : usage.recordings === 0 ? "No recordings."
            : `${usage.recordings} ${usage.recordings === 1 ? "recording" : "recordings"}, ${formatAttachmentSize(usage.audioBytes)} of audio.`}
      </p>
      <div className="settings-section-heading-actions">
        <button type="button" className="btn" disabled={busy || !usage?.audioBytes} aria-describedby={ids.usage} onClick={() => confirm("audio")}>Delete All Audio</button>
        <button type="button" className="btn" disabled={busy || !usage?.recordings} aria-describedby={ids.usage} onClick={() => confirm("recordings")}>Delete All Recordings</button>
      </div>
      {confirming ? (
        <InlineConfirm
          ariaLabel={confirming === "audio" ? "Confirm Delete All Audio" : "Confirm Delete All Recordings"}
          title={confirming === "audio" ? "Delete all recording audio?" : "Delete all recordings?"}
          cancelLabel="Cancel"
          onCancel={() => setConfirming(null)}
          disabled={busy}
          focusCancel
          actions={[{ label: confirming === "audio" ? "Delete All Audio" : "Delete All Recordings", className: "btn-danger", onClick: () => void deleteAll(confirming) }]}
        >
          {confirming === "audio"
            ? "The audio files are removed from this Mac. The recordings stay listed, and this can't be undone."
            : "Every recording and its audio is removed from this Mac. This can't be undone."}
        </InlineConfirm>
      ) : null}
      {message ? <p role="status" className="settings-hint">{message}</p> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

/** Settings → Meeting Recording → Transcription & Vocabulary. */
export function MeetingTranscriptionSettings({ capabilities, onOpenAiSettings }: {
  capabilities: RecordingCapabilities;
  onOpenAiSettings?(): void;
}) {
  const { settings, error, setError, change } = useRecordingSettingsState();
  const [pendingModel, setPendingModel] = useState<TranscriptionModelName | null>(null);
  const [loadingSuggestions, setLoadingSuggestions] = useState(false);
  const [savingVocab, setSavingVocab] = useState(false);
  const [savingCloud, setSavingCloud] = useState(false);
  const [deletingModel, setDeletingModel] = useState<TranscriptionModelName | null>(null);
  const [newVocabTerm, setNewVocabTerm] = useState("");
  const [vocabFeedback, setVocabFeedback] = useState<string | null>(null);
  const [confirmingClearVocab, setConfirmingClearVocab] = useState(false);
  const [suggestions, setSuggestions] = useState<VocabularySuggestion[] | null>(null);
  const [selectedSuggestions, setSelectedSuggestions] = useState<string[]>([]);
  const [confirmingCloud, setConfirmingCloud] = useState(false);
  const [cloudModelDraft, setCloudModelDraft] = useState("");
  const cloudModelPendingRef = useRef<string | null>(null);
  const changeRef = useRef(change);
  changeRef.current = change;
  const models = useTranscriptionModels();
  const radioGroupName = useId();
  const ids = {
    model: useId(), language: useId(), languageControl: useId(), during: useId(), vocab: useId(),
    cloud: useId(), cloudModel: useId(), cloudModelControl: useId(), cloudDiarize: useId(), cloudDefault: useId(),
  };

  const storedCloudModel = settings?.cloudTranscriptionModel ?? "";
  useEffect(() => {
    if (cloudModelPendingRef.current === null) setCloudModelDraft(storedCloudModel);
  }, [storedCloudModel]);

  const commitCloudModel = useCallback(() => {
    const edit = cloudModelPendingRef.current;
    if (edit === null) return;
    cloudModelPendingRef.current = null;
    void changeRef.current({ cloudTranscriptionModel: edit });
  }, []);

  useEffect(() => commitCloudModel, [commitCloudModel]);

  const modelCommand = async (model: TranscriptionModelName, run: () => Promise<void>) => {
    setPendingModel(model);
    setError(null);
    try { await run(); } catch (reason) { setError(errorMessage(reason)); } finally { setPendingModel(null); }
  };

  const addCustomVocab = async (termsToAdd: string[]): Promise<CustomVocabMergeResult | null> => {
    if (!settings) return null;
    const result = mergeCustomVocab(settings.customVocabulary, termsToAdd);
    setVocabFeedback(formatVocabFeedback(result));
    if (result.added === 0) return result;
    setSavingVocab(true);
    try {
      await change({ customVocabulary: result.next });
    } finally {
      setSavingVocab(false);
    }
    return result;
  };

  const submitNewVocab = async () => {
    if (!newVocabTerm.trim()) return;
    const result = await addCustomVocab(parseCustomVocabInput(newVocabTerm));
    if (result && result.added > 0) setNewVocabTerm("");
  };

  const removeCustomVocab = async (term: string) => {
    if (!settings) return;
    setVocabFeedback(null);
    setSavingVocab(true);
    try {
      await change({ customVocabulary: settings.customVocabulary.filter((item) => item !== term) });
    } finally {
      setSavingVocab(false);
    }
  };

  const clearCustomVocab = async () => {
    if (!settings) return;
    setVocabFeedback(null);
    setConfirmingClearVocab(false);
    setSavingVocab(true);
    try {
      await change({ customVocabulary: [] });
    } finally {
      setSavingVocab(false);
    }
  };

  const loadVocabSuggestions = async () => {
    setLoadingSuggestions(true);
    setError(null);
    setVocabFeedback(null);
    try {
      const found = await mailClient.suggestRecordingVocabulary();
      setSuggestions(found);
      setSelectedSuggestions([]);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setLoadingSuggestions(false);
    }
  };

  const runCloudSave = async (run: () => Promise<void>) => {
    setSavingCloud(true);
    try {
      await run();
    } finally {
      setSavingCloud(false);
    }
  };

  const toggleCloudTranscription = async (on: boolean) => {
    if (!settings) return;
    setError(null);
    if (!on) {
      setConfirmingCloud(false);
      cloudModelPendingRef.current = null;
      await runCloudSave(() => change({ cloudTranscriptionEnabled: false, cloudTranscriptionDefault: false }));
      return;
    }
    setConfirmingCloud(true);
  };

  const confirmEnableCloud = async () => {
    if (!settings) return;
    const activeProvider = readAiProvider();
    const provider = settings.cloudTranscriptionProvider ?? (isCloudTranscriptionProvider(activeProvider) ? activeProvider : null);
    const model = settings.cloudTranscriptionModel.trim() || defaultCloudTranscriptionModel(provider ?? "openrouter") || "microsoft/mai-transcribe-2";
    const endpoint = settings.cloudTranscriptionEndpoint ?? (activeProvider === "custom" ? readAiEndpoint().trim() || null : null);
    setConfirmingCloud(false);
    cloudModelPendingRef.current = null;
    await runCloudSave(() => change({
      cloudTranscriptionEnabled: true,
      cloudTranscriptionDisclosureAcknowledged: true,
      cloudTranscriptionProvider: provider,
      cloudTranscriptionModel: model,
      cloudTranscriptionEndpoint: endpoint,
    }));
  };

  const speechAnalyzerAvailable = capabilities.speechAnalyzer;
  const selectedEngineValue: "apple" | TranscriptionModelName = speechAnalyzerAvailable && settings?.transcriptionEngine !== "whisper"
    ? "apple"
    : (settings?.transcriptionModel ?? "fast");
  const cloudEnabled = Boolean(settings?.cloudTranscriptionEnabled);
  const cloudIsDefault = Boolean(cloudEnabled && settings?.cloudTranscriptionDefault);
  const isWhisperFastSelected = selectedEngineValue === "fast";
  const languageLocked = isWhisperFastSelected && !cloudEnabled;
  const displayedLanguage = languageLocked ? "en" : (settings?.transcriptionLanguage ?? "");
  const vocabCount = settings?.customVocabulary.length ?? 0;
  const atVocabLimit = vocabCount >= CUSTOM_VOCAB_MAX_TERMS;
  const providerConfigured = isCloudTranscriptionProvider(readAiProvider())
    || Boolean(settings?.cloudTranscriptionProvider && isCloudTranscriptionProvider(settings.cloudTranscriptionProvider));

  const switchPromptTarget = speechAnalyzerAvailable
    ? "Whisper Accurate or Apple On-Device"
    : "Whisper Accurate";

  return (
    <section className="settings-section transcription-settings" aria-label="Transcription & Vocabulary">
      <h3>On-Device Transcription</h3>
      {cloudIsDefault ? (
        <p className="settings-inline-note" role="note">
          Cloud Transcription is set as default. These on-device settings apply when transcribing locally or if Cloud Transcription is unavailable.
        </p>
      ) : null}
      <div className="settings-item">
        <div
          className="transcription-models"
          role="radiogroup"
          aria-label="Transcription Model"
          aria-describedby={ids.model}
        >
          {speechAnalyzerAvailable ? (
            <TranscriptionModelRow
              radioGroupName={radioGroupName}
              value="apple"
              label="Apple On-Device"
              meta="Built into macOS · No download needed"
              status="Ready"
              statusTone="ready"
              selected={selectedEngineValue === "apple"}
              cloudIsDefault={cloudIsDefault}
              selectDisabled={!settings}
              onSelect={() => void change({ transcriptionEngine: "speechAnalyzer" })}
            />
          ) : null}
          {MODELS.map((option) => {
            const selected = selectedEngineValue === option.value;
            const modelStatus = models?.find((item) => item.model === option.value);
            const { text, tone } = whisperModelStatus(modelStatus, selected);
            return (
              <TranscriptionModelRow
                key={option.value}
                radioGroupName={radioGroupName}
                value={option.value}
                label={option.label}
                meta={`${modelStatus ? modelSize(modelStatus.bytes) : "…"} · ${option.note}`}
                status={text}
                statusTone={tone}
                selected={selected}
                cloudIsDefault={cloudIsDefault}
                selectDisabled={!settings}
                onSelect={() => {
                  if (speechAnalyzerAvailable) void change({ transcriptionEngine: "whisper", transcriptionModel: option.value });
                  else void change({ transcriptionModel: option.value });
                }}
                model={modelStatus}
                busy={pendingModel === option.value}
                onDownload={() => void modelCommand(option.value, () => mailClient.downloadTranscriptionModel(option.value))}
                onCancel={() => void modelCommand(option.value, () => mailClient.cancelTranscriptionModelDownload(option.value))}
                onDelete={() => setDeletingModel(option.value)}
              />
            );
          })}
        </div>
        <p className="settings-hint" id={ids.model}>
          {speechAnalyzerAvailable
            ? "Recordings are transcribed on this Mac, never uploaded. Apple On-Device needs no download; Whisper Fast and Accurate are optional downloadable models."
            : "Recordings are transcribed on this Mac, never uploaded, once the chosen model is downloaded. Whisper Fast suits English meetings; Whisper Accurate is larger and slower but handles many languages."}
        </p>
      </div>
      {deletingModel ? (
        <InlineConfirm
          ariaLabel="Confirm Delete Model"
          title={`Delete the ${MODELS.find((option) => option.value === deletingModel)?.label} model?`}
          cancelLabel="Cancel"
          onCancel={() => setDeletingModel(null)}
          disabled={pendingModel === deletingModel}
          focusCancel
          actions={[{ label: "Delete Model", className: "btn-danger", onClick: () => void modelCommand(deletingModel, async () => { await mailClient.deleteTranscriptionModel(deletingModel); setDeletingModel(null); }) }]}
        >
          The model file is removed from this Mac. Recordings wait to be transcribed until a model is downloaded again.
        </InlineConfirm>
      ) : null}

      <div className="settings-item">
        <div className="settings-item-control">
          <label htmlFor={ids.languageControl}>Transcription Language</label>
          <select
            id={ids.languageControl}
            value={displayedLanguage}
            disabled={!settings || languageLocked}
            aria-describedby={ids.language}
            onChange={(event) => void change({ transcriptionLanguage: event.target.value || null })}
          >
            {LANGUAGES.map((option) => <option key={option.value || "auto"} value={option.value}>{option.label}</option>)}
          </select>
        </div>
        <p className="settings-hint" id={ids.language}>
          Auto detects the spoken language once near the start of each recording.
        </p>
        {languageLocked ? (
          <p className="settings-inline-note" role="note">
            {settings?.transcriptionLanguage && settings.transcriptionLanguage !== "en"
              ? `Whisper Fast supports English only. Your saved language will apply again if you switch to ${switchPromptTarget}.`
              : `Whisper Fast supports English only. Switch to ${switchPromptTarget} to choose another language.`}
          </p>
        ) : isWhisperFastSelected && cloudEnabled && (settings?.transcriptionLanguage ?? "") !== "en" ? (
          <p className="settings-inline-note" role="note">
            Whisper Fast supports English only; your selected language applies to Cloud Transcription.
          </p>
        ) : null}
      </div>

      <div className="settings-item">
        <label className="settings-switch">
          <span>Transcribe During Meetings</span>
          <input type="checkbox" checked={settings?.transcribeDuringMeetings ?? true} disabled={!settings} aria-describedby={ids.during}
            onChange={(event) => void change({ transcribeDuringMeetings: event.target.checked })} />
        </label>
        <p className="settings-hint" id={ids.during}>
          Transcribes in the background while a meeting is recorded, so the transcript is ready soon after you stop. Turn off to keep your Mac's memory and processor free during meetings.
        </p>
      </div>

      <div className="settings-section-heading-row">
        <h3>Custom Vocabulary</h3>
        {vocabCount > 0 ? (
          <div className="settings-section-heading-actions">
            <span className="settings-hint recording-vocab-count">{`${vocabCount} / ${CUSTOM_VOCAB_MAX_TERMS}`}</span>
            {vocabCount >= 2 ? (
              <button
                type="button"
                className="btn-link"
                disabled={savingVocab}
                onClick={() => setConfirmingClearVocab(true)}
              >Clear All</button>
            ) : null}
          </div>
        ) : null}
      </div>
      <div className="settings-item">
        <div className="recording-vocab-add">
          <input
            type="text"
            aria-label="Custom Vocabulary term"
            placeholder="Add product, acronym, or name (comma-separated)"
            value={newVocabTerm}
            maxLength={CUSTOM_VOCAB_MAX_TERM_CHARS * 4}
            disabled={!settings || savingVocab || atVocabLimit}
            aria-describedby={ids.vocab}
            onChange={(event) => {
              setNewVocabTerm(event.target.value);
              if (vocabFeedback) setVocabFeedback(null);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && newVocabTerm.trim()) {
                event.preventDefault();
                void submitNewVocab();
              }
            }}
          />
          <button
            type="button"
            className="btn"
            disabled={!settings || savingVocab || atVocabLimit || !newVocabTerm.trim()}
            onClick={() => void submitNewVocab()}
          >Add</button>
        </div>
        <div className="recording-vocab-toolbar">
          <button
            type="button"
            className="btn btn-sm"
            disabled={!settings || loadingSuggestions || atVocabLimit}
            onClick={() => void loadVocabSuggestions()}
          >{loadingSuggestions ? "Scanning Sent Mail…" : "Suggest from Sent Mail"}</button>
          {vocabFeedback ? <span className="settings-hint" role="status">{vocabFeedback}</span> : null}
        </div>
        <p className="settings-hint" id={ids.vocab}>
          {speechAnalyzerAvailable
            ? "Terms you approve here help Apple On-Device, Whisper, and Cloud Transcription spell names, acronyms, and project vocabulary accurately."
            : "Terms you approve here help Whisper and Cloud Transcription spell names, acronyms, and project vocabulary accurately."}
        </p>
      </div>
      {confirmingClearVocab ? (
        <InlineConfirm
          ariaLabel="Confirm Clear Custom Vocabulary"
          title="Clear all custom vocabulary?"
          cancelLabel="Cancel"
          onCancel={() => setConfirmingClearVocab(false)}
          disabled={savingVocab}
          focusCancel
          actions={[{ label: "Clear All", className: "btn-danger", onClick: () => void clearCustomVocab() }]}
        >
          {`All ${vocabCount} saved vocabulary terms will be removed.`}
        </InlineConfirm>
      ) : null}
      {settings && settings.customVocabulary.length > 0 ? (
        <ul className="recording-vocab-list" aria-label="Custom Vocabulary list">
          {settings.customVocabulary.map((term) => (
            <li key={term} className="recording-vocab-chip">
              <span>{term}</span>
              <button
                type="button"
                className="btn-link"
                aria-label={`Remove ${term}`}
                disabled={savingVocab}
                onClick={() => void removeCustomVocab(term)}
              >×</button>
            </li>
          ))}
        </ul>
      ) : null}
      {suggestions ? (
        <div className="recording-vocab-suggestions" role="region" aria-label="Sent Mail Vocabulary Suggestions">
          {suggestions.length === 0 ? (
            <>
              <p className="settings-hint">No recurring vocabulary terms were found in recent Sent mail.</p>
              <div className="transcription-model-actions">
                <button type="button" className="btn" onClick={() => setSuggestions(null)}>Close</button>
              </div>
            </>
          ) : (
            <>
              <p className="settings-hint">Select recurring terms from your Sent mail to add:</p>
              <div className="recording-vocab-suggestion-list">
                {suggestions.map((item) => {
                  const checked = selectedSuggestions.includes(item.term);
                  return (
                    <label key={item.term} className="recording-vocab-suggestion">
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={(event) => {
                          if (event.target.checked) setSelectedSuggestions((cur) => [...cur, item.term]);
                          else setSelectedSuggestions((cur) => cur.filter((value) => value !== item.term));
                        }}
                      />
                      <span>{item.term}</span>
                      <span className="settings-hint">{item.threadCount} threads</span>
                    </label>
                  );
                })}
              </div>
              <div className="transcription-model-actions">
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={savingVocab || selectedSuggestions.length === 0}
                  onClick={() => {
                    const chosen = [...selectedSuggestions];
                    setSuggestions(null);
                    setSelectedSuggestions([]);
                    void addCustomVocab(chosen);
                  }}
                >{selectedSuggestions.length > 0 ? `Add Selected (${selectedSuggestions.length})` : "Add Selected"}</button>
                {selectedSuggestions.length < suggestions.length ? (
                  <button
                    type="button"
                    className="btn"
                    onClick={() => setSelectedSuggestions(suggestions.map((item) => item.term))}
                  >Select All</button>
                ) : null}
                {selectedSuggestions.length > 0 ? (
                  <button
                    type="button"
                    className="btn"
                    onClick={() => setSelectedSuggestions([])}
                  >Clear</button>
                ) : null}
                <button
                  type="button"
                  className="btn"
                  onClick={() => { setSuggestions(null); setSelectedSuggestions([]); }}
                >Cancel</button>
              </div>
            </>
          )}
        </div>
      ) : null}

      <h3>Cloud Transcription</h3>
      <div className="settings-item">
        <label className="settings-switch">
          <span>Enable Cloud Transcription</span>
          <input type="checkbox" checked={cloudEnabled} disabled={!settings || savingCloud} aria-describedby={ids.cloud}
            onChange={(event) => void toggleCloudTranscription(event.target.checked)} />
        </label>
        <p className="settings-hint" id={ids.cloud}>
          Lets you transcribe or re-transcribe recordings using OpenRouter (Microsoft MAI-Transcribe-2) or an OpenAI-compatible audio provider, sending separate audio tracks and your reviewed Custom Vocabulary within your daily AI budget.
        </p>
      </div>
      {confirmingCloud ? (
        <InlineConfirm
          ariaLabel="Confirm Cloud Transcription"
          title="Enable cloud transcription?"
          cancelLabel="Cancel"
          onCancel={() => setConfirmingCloud(false)}
          disabled={savingCloud}
          focusCancel
          actions={[{ label: "Enable Cloud Transcription", className: "btn-primary", onClick: () => void confirmEnableCloud() }]}
        >
          When you use Cloud Transcription, separate microphone and system audio tracks and your reviewed Custom Vocabulary are sent to your configured AI provider and counted toward your daily AI budget.
        </InlineConfirm>
      ) : null}
      {settings?.cloudTranscriptionEnabled ? (
        <>
          {!providerConfigured ? (
            <p className="settings-inline-note" role="note">
              Configure OpenRouter, OpenAI, Fireworks, or Custom Endpoint in AI settings to use Cloud Transcription.
              {onOpenAiSettings ? <> <button type="button" className="btn-link" onClick={onOpenAiSettings}>AI Provider</button></> : null}
            </p>
          ) : null}
          <div className="settings-item">
            <div className="settings-item-control">
              <label htmlFor={ids.cloudModelControl}>Cloud Transcription Model</label>
              <input
                id={ids.cloudModelControl}
                className="transcription-cloud-model-input"
                type="text"
                value={cloudModelDraft}
                placeholder="microsoft/mai-transcribe-2"
                aria-describedby={ids.cloudModel}
                onChange={(event) => {
                  const value = event.target.value;
                  setCloudModelDraft(value);
                  cloudModelPendingRef.current = value;
                }}
                onBlur={commitCloudModel}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    commitCloudModel();
                  }
                }}
              />
            </div>
            <p className="settings-hint" id={ids.cloudModel}>
              Recommended on OpenRouter: microsoft/mai-transcribe-2 (low cost, custom vocabulary biasing, and remote speaker diarization).
            </p>
          </div>
          <div className="settings-item">
            <label className="settings-switch">
              <span>Diarize Remote Speakers</span>
              <input type="checkbox" checked={settings.cloudTranscriptionDiarize} disabled={savingCloud} aria-describedby={ids.cloudDiarize}
                onChange={(event) => void runCloudSave(() => change({ cloudTranscriptionDiarize: event.target.checked }))} />
            </label>
            <p className="settings-hint" id={ids.cloudDiarize}>
              Separates remote voices on the computer-audio track (Others (Speaker 1), Others (Speaker 2)) while keeping your microphone track as You.
            </p>
          </div>
          <div className="settings-item">
            <label className="settings-switch">
              <span>Use Cloud by Default</span>
              <input type="checkbox" checked={settings.cloudTranscriptionDefault} disabled={savingCloud} aria-describedby={ids.cloudDefault}
                onChange={(event) => {
                  const activeProvider = readAiProvider();
                  const provider = settings.cloudTranscriptionProvider ?? (isCloudTranscriptionProvider(activeProvider) ? activeProvider : null);
                  const endpoint = settings.cloudTranscriptionEndpoint ?? (activeProvider === "custom" ? readAiEndpoint().trim() || null : null);
                  void runCloudSave(() => change({ cloudTranscriptionDefault: event.target.checked, cloudTranscriptionProvider: provider, cloudTranscriptionEndpoint: endpoint }));
                }} />
            </label>
            <p className="settings-hint" id={ids.cloudDefault}>
              Automatically uploads finished recordings after you stop recording. Leave off to keep transcription on this Mac and only re-transcribe selected calls from Show Transcript.
            </p>
          </div>
        </>
      ) : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

/** Settings → Meeting Recording → Summaries. */
export function MeetingSummarySettings({ aiConfigured = true, onOpenAiSettings, onChange }: {
  aiConfigured?: boolean;
  onOpenAiSettings?(): void;
  onChange?(): void;
}) {
  const { settings, error, change } = useRecordingSettingsState();
  const storedInstructions = useMeetingSummaryInstructions();
  const [draft, setDraft] = useState(storedInstructions);
  const pendingRef = useRef<string | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const ids = { summarize: useId(), instructions: useId() };

  const commit = useCallback(() => {
    const edit = pendingRef.current;
    if (edit === null) return;
    pendingRef.current = null;
    saveMeetingSummaryInstructions(edit);
    onChangeRef.current?.();
  }, []);

  useEffect(() => commit, [commit]);

  useEffect(() => {
    if (pendingRef.current === null) setDraft(storedInstructions);
  }, [storedInstructions]);

  const applyPreset = (instructions: string) => {
    pendingRef.current = null;
    setDraft(instructions);
    saveMeetingSummaryInstructions(instructions);
    onChangeRef.current?.();
  };

  return (
    <section className="settings-section wide-label-settings writing-profile-settings" aria-label="Summaries">
      <h3>Meeting Recaps</h3>
      {aiConfigured ? null : (
        <p className="settings-hint" role="note">
          Configure an AI provider to summarize recorded meetings.
          {onOpenAiSettings ? <> <button type="button" className="btn-link" onClick={onOpenAiSettings}>AI Provider</button></> : null}
        </p>
      )}
      <label className="settings-switch">
        <span>Summarize with AI</span>
        <input type="checkbox" checked={settings?.summarizeWithAi ?? false} disabled={!settings} aria-describedby={ids.summarize}
          onChange={(event) => void change({ summarizeWithAi: event.target.checked })} />
      </label>
      <p className="settings-hint" id={ids.summarize}>
        Uses your configured AI provider to draft structured meeting notes, decisions, actions, and open questions when a transcription finishes. When off, recordings stop at the transcript.
      </p>

      <h3>Summary Instructions</h3>
      <p className="settings-hint" id={ids.instructions}>
        Customize what AI meeting summaries emphasize and how they are organized. Leave blank for the built-in overview with bold topic headings and bullet takeaways. Instructions sync across your devices, and every fact stays grounded in the transcript.
      </p>
      <div className="recording-summary-presets" role="group" aria-label="Summary instruction presets">
        {MEETING_SUMMARY_PRESETS.map((preset) => (
          <button
            key={preset.id}
            type="button"
            className="btn btn-sm"
            onClick={() => applyPreset(preset.instructions)}
          >{preset.label}</button>
        ))}
      </div>
      <label className="settings-field">
        <span>Custom Summary Instructions</span>
        <textarea
          rows={8}
          value={draft}
          maxLength={MAX_MEETING_SUMMARY_INSTRUCTIONS_CHARS}
          aria-describedby={ids.instructions}
          placeholder="e.g. Lead with key decisions and risks, then group discussion points by topic with concise bullet takeaways."
          onChange={(event) => {
            const value = event.target.value;
            setDraft(value);
            pendingRef.current = value;
          }}
          onBlur={commit}
        />
      </label>
      <small className="settings-hint writing-profile-count">{`${[...draft].length.toLocaleString()} / ${MAX_MEETING_SUMMARY_INSTRUCTIONS_CHARS.toLocaleString()}`}</small>
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}
