import { useEffect, useRef, useState } from "react";
import { Sparkles } from "lucide-react";
import { readAiEndpoint, readAiProvider } from "./aiSettings";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import type { MeetingRecording, RecordingSettings, RecordingTranscript } from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { InlineConfirm } from "./InlineConfirm";
import { defaultCloudTranscriptionModel, formatElapsed, isCloudTranscriptionProvider, speakerLabel, transcriptCopyText } from "./recordingPolicy";

/**
 * A recording's transcript, read-only. Transcript text comes from other
 * people's speech, so it is rendered as text and nothing here can edit it.
 * Delete Recording asks first, then closes the dialog.
 */
export function TranscriptDialog({ recording, onClose, onDeleted }: { recording: MeetingRecording; onClose(): void; onDeleted(): void }) {
  const [transcript, setTranscript] = useState<RecordingTranscript | null>(null);
  const [settings, setSettings] = useState<RecordingSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [statusNote, setStatusNote] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [cloudBusy, setCloudBusy] = useState(false);
  const lines = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let active = true;
    mailClient.recordingTranscript(recording.id).then((found) => { if (active) setTranscript(found); }, (reason: unknown) => { if (active) setError(errorMessage(reason)); });
    mailClient.recordingSettings().then((saved) => { if (active) setSettings(saved); }, () => {});
    return () => { active = false; };
  }, [recording.id]);

  const copy = async () => {
    if (!transcript) return;
    setError(null);
    try {
      await navigator.clipboard.writeText(transcriptCopyText(transcript));
      setStatusNote("Copied to the clipboard.");
    } catch (reason) {
      setStatusNote(null);
      setError(errorMessage(reason));
    }
  };
  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await mailClient.deleteRecording(recording.id);
      onDeleted();
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };
  const cloudRetranscribe = async () => {
    if (!settings) return;
    setError(null);
    setStatusNote(null);
    const activeProvider = readAiProvider();
    const provider = isCloudTranscriptionProvider(activeProvider)
      ? activeProvider
      : settings.cloudTranscriptionProvider;
    if (!provider || !isCloudTranscriptionProvider(provider)) {
      setError("Choose OpenRouter, OpenAI, Fireworks, or Custom Endpoint in AI settings first.");
      return;
    }
    const model = settings.cloudTranscriptionModel.trim() || defaultCloudTranscriptionModel(provider);
    if (!model) {
      setError("Set a cloud transcription model in Settings → Meeting Recording first.");
      return;
    }
    const endpoint = provider === "custom"
      ? (readAiEndpoint().trim() || settings.cloudTranscriptionEndpoint || null)
      : null;
    setCloudBusy(true);
    try {
      const updated = await mailClient.cloudTranscribeRecording(
        recording.id,
        provider,
        model,
        endpoint,
        settings.cloudTranscriptionDiarize,
      );
      setTranscript(updated);
      setStatusNote("Re-transcribed with cloud.");
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setCloudBusy(false);
    }
  };

  const canCloudRetranscribe = Boolean(
    settings?.cloudTranscriptionEnabled
    && settings.cloudTranscriptionDisclosureAcknowledged
    && !recording.audioDeleted,
  );

  return <Modal title="Transcript" className="transcript-modal" initialFocusRef={lines} onClose={onClose}>
    <div className="modal-form">
      {/* The scrolling region takes focus first so the arrow keys scroll a long transcript. */}
      <div ref={lines} className="transcript-lines" role="region" aria-label="Transcript text" tabIndex={0}>
        {transcript === null && !error ? <p className="modal-form-hint" role="status">Loading the transcript…</p> : null}
        {transcript && transcript.lines.length === 0 ? <p className="modal-form-hint">Nothing was said that could be transcribed.</p> : null}
        {transcript?.lines.map((line, index) => (
          <p key={index} className="transcript-line">
            <span className="transcript-line-time">{formatElapsed(line.startMs)}</span>
            {transcript.speakerLabels && line.speaker ? <span className="transcript-line-speaker">{speakerLabel(line.speaker, line.speakerLabel)}</span> : null}
            <span className="transcript-line-text">{line.text}</span>
          </p>
        ))}
      </div>
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      {statusNote ? <p className="modal-form-hint" role="status">{statusNote}</p> : null}
      {confirming ? <InlineConfirm
        ariaLabel="Confirm Delete Recording"
        title="Delete this recording?"
        cancelLabel="Cancel"
        onCancel={() => setConfirming(false)}
        disabled={busy}
        focusCancel
        actions={[{ label: "Delete Recording", className: "btn-danger", onClick: () => void remove() }]}
      >
        The recording, its audio, and its transcript are removed from this Mac. This can't be undone.
      </InlineConfirm> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" disabled={busy || cloudBusy || confirming} onClick={() => { setStatusNote(null); setConfirming(true); }}>Delete Recording</button>
        {canCloudRetranscribe ? (
          <button type="button" className="btn" disabled={!transcript || busy || cloudBusy || confirming} onClick={() => void cloudRetranscribe()}>
            <Sparkles size={ICON_SIZE.sm} aria-hidden="true" />{cloudBusy ? "Re-transcribing…" : "Re-transcribe with Cloud"}
          </button>
        ) : null}
        <button type="button" className="btn" disabled={!transcript || cloudBusy} onClick={() => void copy()}>Copy</button>
        <button type="button" className="btn btn-primary" onClick={onClose}>Close</button>
      </div>
    </div>
  </Modal>;
}
