import { useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { errorMessage } from "./errors";
import { snoozePresets, toLocalDateTime } from "./snoozeTimes";

export function SnoozeDialog({ count, initialUntil, onSave, onClose }: {
  count: number;
  initialUntil?: string | null;
  onSave(until: string): Promise<void>;
  onClose(): void;
}) {
  const [value, setValue] = useState(() => toLocalDateTime(initialUntil ? new Date(initialUntil) : snoozePresets(new Date()).find((preset) => preset.label === "Tomorrow")!.at));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const dateInput = useRef<HTMLInputElement>(null);
  const save = async (at: Date) => {
    if (busy) return;
    if (!Number.isFinite(at.getTime()) || at.getTime() <= Date.now()) { setError("Choose a future return time."); return; }
    setBusy(true);
    setError("");
    try { await onSave(at.toISOString()); onClose(); }
    catch (reason) { setError(errorMessage(reason)); }
    finally { setBusy(false); }
  };
  return <Modal title={initialUntil ? "Reschedule Snooze" : count > 1 ? `Snooze ${count} Conversations` : "Snooze Conversation"} onClose={() => { if (!busy) onClose(); }} initialFocusRef={dateInput}>
    <form className="modal-form" onSubmit={(event) => { event.preventDefault(); void save(new Date(value)); }}>
      <p id="snooze-description" className="modal-form-hint">Removed from Inbox in all your mail apps. Returns when a new reply arrives or at the chosen time while ThreeStrands is running and online. If closed, it catches up when reopened and connected.</p>
      <div className="modal-form-actions">
        {snoozePresets(new Date()).map(({ label, at }) => <button className="btn" type="button" key={label} disabled={busy} onClick={() => { void save(at); }}>{label}</button>)}
      </div>
      <label>Pick date &amp; time
        <input ref={dateInput} type="datetime-local" value={value} disabled={busy} aria-describedby="snooze-description snooze-timezone" onChange={(event) => setValue(event.target.value)} required />
      </label>
      <p id="snooze-timezone" className="modal-form-hint">{Intl.DateTimeFormat().resolvedOptions().timeZone}</p>
      {error ? <p role="alert" className="form-error">{error}</p> : null}
      <div className="modal-form-actions">
        <button className="btn" type="button" disabled={busy} onClick={onClose}>Cancel</button>
        <button className="btn btn-primary" type="submit" disabled={busy}>{busy ? "Saving…" : "Snooze"}</button>
      </div>
    </form>
  </Modal>;
}
