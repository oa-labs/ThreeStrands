import { useEffect, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import type { MeetingRecording, RecordingTranscript } from "./domain";
import { errorMessage } from "./errors";
import { InlineConfirm } from "./InlineConfirm";
import { formatElapsed, speakerLabel, transcriptCopyText } from "./recordingPolicy";

/**
 * A recording's transcript, read-only. Transcript text comes from other
 * people's speech, so it is rendered as text and nothing here can edit it.
 * Delete Recording asks first, then closes the dialog.
 */
export function TranscriptDialog({ recording, onClose, onDeleted }: { recording: MeetingRecording; onClose(): void; onDeleted(): void }) {
  const [transcript, setTranscript] = useState<RecordingTranscript | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const lines = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let active = true;
    mailClient.recordingTranscript(recording.id).then((found) => { if (active) setTranscript(found); }, (reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, [recording.id]);

  const copy = async () => {
    if (!transcript) return;
    setError(null);
    try {
      await navigator.clipboard.writeText(transcriptCopyText(transcript));
      setCopied(true);
    } catch (reason) {
      setCopied(false);
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

  return <Modal title="Transcript" className="transcript-modal" initialFocusRef={lines} onClose={onClose}>
    <div className="modal-form">
      {/* The scrolling region takes focus first so the arrow keys scroll a long transcript. */}
      <div ref={lines} className="transcript-lines" role="region" aria-label="Transcript text" tabIndex={0}>
        {transcript === null && !error ? <p className="modal-form-hint" role="status">Loading the transcript…</p> : null}
        {transcript && transcript.lines.length === 0 ? <p className="modal-form-hint">Nothing was said that could be transcribed.</p> : null}
        {transcript?.lines.map((line, index) => (
          <p key={index} className="transcript-line">
            <span className="transcript-line-time">{formatElapsed(line.startMs)}</span>
            {transcript.speakerLabels && line.speaker ? <span className="transcript-line-speaker">{speakerLabel(line.speaker)}</span> : null}
            <span className="transcript-line-text">{line.text}</span>
          </p>
        ))}
      </div>
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      {copied ? <p className="modal-form-hint" role="status">Copied to the clipboard.</p> : null}
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
        <button type="button" className="btn" disabled={busy || confirming} onClick={() => { setCopied(false); setConfirming(true); }}>Delete Recording</button>
        <button type="button" className="btn" disabled={!transcript} onClick={() => void copy()}>Copy</button>
        <button type="button" className="btn btn-primary" onClick={onClose}>Close</button>
      </div>
    </div>
  </Modal>;
}
