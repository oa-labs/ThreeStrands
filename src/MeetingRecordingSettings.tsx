import { useCallback, useEffect, useId, useState } from "react";
import { mailClient } from "./data/client";
import type { AudioInputDevice, KeepAudio, RecordingCapabilities, RecordingSettings, RecordingsUsage, TranscriptionModelName, TranscriptionModelStatus } from "./domain";
import { errorMessage } from "./errors";
import { InlineConfirm } from "./InlineConfirm";
import { announceRecordingSettingsChanged } from "./recordingEvents";
import { formatAttachmentSize } from "./threadPresentation";
import { useTranscriptionModels } from "./useTranscriptionModels";

const KEEP_AUDIO: { value: KeepAudio; label: string }[] = [
  { value: "days7", label: "7 Days" },
  { value: "days30", label: "30 Days" },
  { value: "untilDeleted", label: "Until Deleted" },
  { value: "untilTranscribed", label: "Until Transcribed" },
];

const MODELS: { value: TranscriptionModelName; label: string; note: string }[] = [
  { value: "fast", label: "Fast", note: "English only" },
  { value: "accurate", label: "Accurate", note: "many languages" },
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

/** One transcription model: its size, its state, and the one thing the user can do next. Nothing downloads until Download is pressed. */
function TranscriptionModelRow({ model, label, note, onDownload, onCancel, onDelete, busy }: {
  model: TranscriptionModelStatus | undefined;
  label: string;
  note: string;
  onDownload(): void;
  onCancel(): void;
  onDelete(): void;
  busy: boolean;
}) {
  const id = useId();
  const percent = model ? modelPercent(model) : 0;
  const partial = model && model.receivedBytes > 0 && model.state !== "ready";
  let status = "Checking…";
  if (model) {
    switch (model.state) {
      case "absent": status = partial ? `Partly downloaded, ${percent}%` : "Not downloaded"; break;
      case "downloading": status = `Downloading… ${percent}%`; break;
      case "verifying": status = "Checking the download…"; break;
      case "ready": status = "Downloaded"; break;
      case "failed": status = "Download failed"; break;
    }
  }
  return (
    <div className="transcription-model" role="group" aria-label={`${label} model`} aria-describedby={id}>
      <div className="transcription-model-text" id={id}>
        <span className="transcription-model-name">{label}</span>
        <span className="settings-hint">{model ? modelSize(model.bytes) : "…"}, {note}</span>
        <span className="settings-hint" role="status">{status}</span>
      </div>
      {model?.state === "downloading" || model?.state === "verifying" ? (
        <progress className="transcription-model-progress" max={100} value={model.state === "downloading" ? percent : undefined} aria-label={`${label} model download`} />
      ) : null}
      <div className="transcription-model-actions">
        {model?.state === "absent" ? <button type="button" className="btn" disabled={busy} onClick={onDownload}>Download</button> : null}
        {model?.state === "failed" ? <button type="button" className="btn" disabled={busy} onClick={onDownload}>Try Again</button> : null}
        {model?.state === "downloading" ? <button type="button" className="btn" disabled={busy} onClick={onCancel}>Cancel</button> : null}
        {model?.state === "ready" ? <button type="button" className="btn" disabled={busy} onClick={onDelete}>Delete Model</button> : null}
      </div>
      {model?.state === "failed" && model.error ? <p className="form-error" role="alert">{model.error}</p> : null}
    </div>
  );
}

/** Settings → Meeting Recording. Everything saves as it changes. */
export function MeetingRecordingSettings({ capabilities }: { capabilities: RecordingCapabilities }) {
  const [settings, setSettings] = useState<RecordingSettings | null>(null);
  const [usage, setUsage] = useState<RecordingsUsage | null>(null);
  const [devices, setDevices] = useState<AudioInputDevice[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [permissionNote, setPermissionNote] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<Confirming>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [deletingModel, setDeletingModel] = useState<TranscriptionModelName | null>(null);
  const models = useTranscriptionModels();
  const ids = {
    record: useId(), microphone: useId(), keep: useId(), remind: useId(),
    usage: useId(), model: useId(), language: useId(), during: useId(),
  };

  const loadUsage = useCallback(async () => setUsage(await mailClient.recordingsUsage()), []);
  useEffect(() => {
    let active = true;
    void Promise.all([mailClient.recordingSettings(), mailClient.recordingsUsage(), mailClient.recordingInputDevices()]).then(([saved, used, inputs]) => {
      if (!active) return;
      setSettings(saved);
      setUsage(used);
      setDevices(inputs);
    }, (reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, []);

  const save = async (next: RecordingSettings) => {
    await mailClient.setRecordingSettings(next);
    setSettings(next);
    announceRecordingSettingsChanged();
  };
  const change = async (update: Partial<RecordingSettings>) => {
    if (!settings) return;
    setError(null);
    try { await save({ ...settings, ...update }); } catch (reason) { setError(errorMessage(reason)); }
  };
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
  const modelCommand = async (run: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try { await run(); } catch (reason) { setError(errorMessage(reason)); } finally { setBusy(false); }
  };
  const confirm = (kind: Confirming) => { setMessage(null); setConfirming(kind); };

  const system = capabilities.systemAudio;
  const activeDevice = settings?.microphoneDeviceUid
    ? devices.find((device) => device.uid === settings.microphoneDeviceUid)
    : devices.find((device) => device.isDefault);
  const speechAnalyzerAvailable = capabilities.speechAnalyzer;
  const selectedEngineValue = speechAnalyzerAvailable && settings?.transcriptionEngine !== "whisper"
    ? "apple"
    : (settings?.transcriptionModel ?? "fast");
  return (
    <section className="settings-section wide-label-settings" aria-label="Meeting Recording">
      <h3>Recording</h3>
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

      <label className="settings-field settings-field-row">
        <span>Transcription Model</span>
        <select value={selectedEngineValue} disabled={!settings} aria-describedby={ids.model}
          onChange={(event) => {
            const value = event.target.value;
            if (value === "apple") void change({ transcriptionEngine: "speechAnalyzer" });
            else if (speechAnalyzerAvailable) void change({ transcriptionEngine: "whisper", transcriptionModel: value as TranscriptionModelName });
            else void change({ transcriptionModel: value as TranscriptionModelName });
          }}>
          {speechAnalyzerAvailable ? <option value="apple">Apple On-Device</option> : null}
          {MODELS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </select>
      </label>
      <p className="settings-hint settings-field-detail" id={ids.model}>
        {speechAnalyzerAvailable
          ? "Recordings are transcribed on this Mac, never uploaded. Apple On-Device needs no download; Whisper Fast and Accurate are optional downloadable models."
          : "Recordings are transcribed on this Mac, never uploaded, once the chosen model is downloaded. Fast suits English meetings; Accurate is larger and slower but handles many languages."}
      </p>
      <label className="settings-field settings-field-row">
        <span>Transcription Language</span>
        <select value={settings?.transcriptionLanguage ?? ""} disabled={!settings} aria-describedby={ids.language}
          onChange={(event) => void change({ transcriptionLanguage: event.target.value || null })}>
          {LANGUAGES.map((option) => <option key={option.value || "auto"} value={option.value}>{option.label}</option>)}
        </select>
      </label>
      <p className="settings-hint settings-field-detail" id={ids.language}>
        Auto detects the spoken language once near the start of each recording. Whisper Fast supports English only.
      </p>
      <div className="settings-field-detail transcription-models">
        {MODELS.map((option) => (
          <TranscriptionModelRow
            key={option.value}
            model={models?.find((model) => model.model === option.value)}
            label={option.label}
            note={option.note}
            busy={busy}
            onDownload={() => void modelCommand(() => mailClient.downloadTranscriptionModel(option.value))}
            onCancel={() => void modelCommand(() => mailClient.cancelTranscriptionModelDownload(option.value))}
            onDelete={() => setDeletingModel(option.value)}
          />
        ))}
      </div>
      {deletingModel ? (
        <InlineConfirm
          ariaLabel="Confirm Delete Model"
          title={`Delete the ${MODELS.find((option) => option.value === deletingModel)?.label} model?`}
          cancelLabel="Cancel"
          onCancel={() => setDeletingModel(null)}
          disabled={busy}
          focusCancel
          actions={[{ label: "Delete Model", className: "btn-danger", onClick: () => void modelCommand(async () => { await mailClient.deleteTranscriptionModel(deletingModel); setDeletingModel(null); }) }]}
        >
          The model file is removed from this Mac. Recordings wait to be transcribed until a model is downloaded again.
        </InlineConfirm>
      ) : null}

      <label className="settings-switch">
        <span>Transcribe During Meetings</span>
        <input type="checkbox" checked={settings?.transcribeDuringMeetings ?? true} disabled={!settings} aria-describedby={ids.during}
          onChange={(event) => void change({ transcribeDuringMeetings: event.target.checked })} />
      </label>
      <p className="settings-hint" id={ids.during}>
        Transcribes in the background while a meeting is recorded, so the transcript is ready soon after you stop. Turn off to keep your Mac's memory and processor free during meetings.
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
