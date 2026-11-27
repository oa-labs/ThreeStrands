import { useCallback, useEffect, useId, useState } from "react";
import { mailClient } from "./data/client";
import type { KeepAudio, RecordingCapabilities, RecordingSettings, RecordingsUsage } from "./domain";
import { errorMessage } from "./errors";
import { InlineConfirm } from "./InlineConfirm";
import { announceRecordingSettingsChanged } from "./recordingEvents";
import { formatAttachmentSize } from "./threadPresentation";

const KEEP_AUDIO: { value: KeepAudio; label: string }[] = [
  { value: "days7", label: "7 Days" },
  { value: "days30", label: "30 Days" },
  { value: "untilDeleted", label: "Until Deleted" },
  { value: "untilTranscribed", label: "Until Transcribed" },
];

const MICROPHONE_DENIED = "Microphone access is off for ThreeStrands. Turn it on in System Settings → Privacy & Security → Microphone, then try again.";
const MICROPHONE_RESTRICTED = "Microphone access is restricted on this Mac, so ThreeStrands can't record. Ask whoever manages this Mac to allow it in System Settings → Privacy & Security → Microphone.";

type Confirming = "audio" | "recordings" | null;

/** Settings → Meeting Recording. Everything saves as it changes. */
export function MeetingRecordingSettings({ capabilities }: { capabilities: RecordingCapabilities }) {
  const [settings, setSettings] = useState<RecordingSettings | null>(null);
  const [usage, setUsage] = useState<RecordingsUsage | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [permissionNote, setPermissionNote] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<Confirming>(null);
  const [message, setMessage] = useState<string | null>(null);
  const ids = { record: useId(), keep: useId(), remind: useId(), usage: useId() };

  const loadUsage = useCallback(async () => setUsage(await mailClient.recordingsUsage()), []);
  useEffect(() => {
    let active = true;
    void Promise.all([mailClient.recordingSettings(), mailClient.recordingsUsage()]).then(([saved, used]) => {
      if (!active) return;
      setSettings(saved);
      setUsage(used);
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
  const confirm = (kind: Confirming) => { setMessage(null); setConfirming(kind); };

  const system = capabilities.systemAudio;
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
        <span>Keep Audio</span>
        <select value={settings?.keepAudio ?? "days7"} disabled={!settings} aria-describedby={ids.keep}
          onChange={(event) => void change({ keepAudio: event.target.value as KeepAudio })}>
          {KEEP_AUDIO.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
        </select>
      </label>
      <p className="settings-hint settings-field-detail" id={ids.keep}>
        How long a recording's audio stays on this Mac before ThreeStrands deletes it. Until Transcribed deletes it once its transcript exists.
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
