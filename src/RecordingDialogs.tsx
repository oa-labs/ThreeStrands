import { useRef, useState, type KeyboardEvent } from "react";
import { Modal } from "./AppChrome";
import { useRecording } from "./recordingContext";

/**
 * The first start on this device. It explains the legal and practical side
 * before anything is captured; Start Recording both records that it was shown
 * and starts.
 */
export function RecordingConsentDialog({ onAccept, onCancel }: { onAccept(): Promise<void>; onCancel(): void }) {
  const [busy, setBusy] = useState(false);
  const start = useRef<HTMLButtonElement>(null);
  const form = useRef<HTMLFormElement>(null);
  const submit = async () => {
    if (busy) return;
    setBusy(true);
    try { await onAccept(); } finally { setBusy(false); }
  };
  const submitShortcut = (event: KeyboardEvent<HTMLFormElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      form.current?.requestSubmit();
    }
  };
  return <Modal title="Recording Meetings" initialFocusRef={start} onClose={onCancel}>
    <form ref={form} className="modal-form" onKeyDown={submitShortcut} onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <p className="modal-form-hint">
        Recording laws differ by place, and some require everyone's consent. Tell the people in the meeting that you are recording it before you start.
      </p>
      <p className="modal-form-hint">
        ThreeStrands records your microphone and your Mac's audio. The audio stays on this Mac and is deleted after the time you choose in Settings → Meeting Recording.
        ThreeStrands never posts to a meeting's chat or invitation for you.
      </p>
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onCancel}>Cancel</button>
        <button ref={start} type="submit" className="btn btn-primary" disabled={busy}>Start Recording</button>
      </div>
    </form>
  </Modal>;
}

/** Quitting while recording asks first; Cancel is where focus starts, so Enter never quits by accident. */
export function QuitRecordingDialog({ onConfirm, onCancel }: { onConfirm(): Promise<void>; onCancel(): void }) {
  const [busy, setBusy] = useState(false);
  const cancel = useRef<HTMLButtonElement>(null);
  return <Modal title="Stop Recording and Quit?" initialFocusRef={cancel} onClose={onCancel}>
    <div className="modal-form">
      <p className="modal-form-hint">A meeting is still being recorded. Quitting stops the recording and saves the audio recorded so far.</p>
      <div className="modal-form-actions">
        <button ref={cancel} type="button" className="btn" onClick={onCancel}>Cancel</button>
        <button type="button" className="btn btn-primary" disabled={busy} onClick={() => { setBusy(true); void onConfirm().finally(() => setBusy(false)); }}>Stop and Quit</button>
      </div>
    </div>
  </Modal>;
}

/** The recording dialogs App shows: first-use consent, and the quit confirmation. */
export function RecordingDialogs() {
  const recording = useRecording();
  return <>
    {recording.consentPending ? <RecordingConsentDialog onAccept={recording.acceptConsent} onCancel={recording.cancelConsent} /> : null}
    {recording.quitRequested ? <QuitRecordingDialog onConfirm={recording.confirmQuit} onCancel={recording.cancelQuit} /> : null}
  </>;
}
