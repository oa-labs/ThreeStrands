import { useEffect, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import type { ScheduleSelection, ScheduleTimeChoice } from "./correspondence";
import { errorMessage } from "./errors";

export function formatScheduledTime(at: number, timeZone: string) {
  return `${new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short", timeZone }).format(at)} (${timeZone})`;
}

export function ScheduleSendPicker({ onClose, onSchedule, initialAt, initialZone }: {
  onClose(): void;
  onSchedule(selection: ScheduleSelection): Promise<void>;
  initialAt?: number;
  initialZone?: string;
}) {
  const initial = new Date(initialAt ?? Date.now() + 60 * 60 * 1000);
  const [timeZone, setTimeZone] = useState(initialZone ?? Intl.DateTimeFormat().resolvedOptions().timeZone);
  const [localTime, setLocalTime] = useState(() => {
    const parts = new Intl.DateTimeFormat("sv-SE", { timeZone: initialZone ?? Intl.DateTimeFormat().resolvedOptions().timeZone, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).formatToParts(initial);
    const part = (type: string) => parts.find((p) => p.type === type)?.value;
    return `${part("year")}-${part("month")}-${part("day")}T${part("hour")}:${part("minute")}`;
  });
  const [choices, setChoices] = useState<ScheduleTimeChoice[]>([]);
  const [offset, setOffset] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const submitting = useRef(false);
  const [info, setInfo] = useState<{deviceName: string; sharing: boolean} | null>(null);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { let active = true; void mailClient.schedulingInfo().then((v) => { if (active) setInfo(v); }).catch((e: unknown) => { if (active) setError(errorMessage(e)); }); return () => { active = false; }; }, []);
  useEffect(() => {
    let active = true;
    setChoices([]); setOffset(""); setError("");
    void mailClient.scheduleChoices(localTime, timeZone).then((v) => {
      if (active) { setChoices(v); if (v.length === 1) setOffset(String(v[0].offsetSeconds)); }
    }).catch((e: unknown) => { if (active) setError(errorMessage(e)); });
    return () => { active = false; };
  }, [localTime, timeZone]);
  const selected = choices.find((v) => String(v.offsetSeconds) === offset);
  return <Modal title={initialAt ? "Reschedule email" : "Send later"} initialFocusRef={input} onClose={() => { if (!submitting.current) onClose(); }}>
    <form className="schedule-send-form" onSubmit={(event) => {
      event.preventDefault();
      if (!selected || submitting.current) return;
      submitting.current = true; setBusy(true); setError("");
      void onSchedule({ localTime, timeZone, offsetSeconds: selected.offsetSeconds }).catch((e: unknown) => setError(errorMessage(e))).finally(() => { submitting.current = false; setBusy(false); });
    }}>
      <p>Sends from this computer{info ? ` (${info.deviceName})` : ""}. Keep ThreeStrands running and this computer awake and connected.</p>
      <label>Date and time<input ref={input} type="datetime-local" aria-label="Scheduled date and time" value={localTime} disabled={busy} onChange={(e) => setLocalTime(e.target.value)} required /></label>
      <label>Timezone<input aria-label="Schedule timezone" value={timeZone} disabled={busy} onChange={(e) => setTimeZone(e.target.value)} required /></label>
      {choices.length > 1 && <label>This time occurs twice. Choose an offset<select aria-label="UTC offset" value={offset} disabled={busy} onChange={(e) => setOffset(e.target.value)} required>
        <option value="">Choose an offset</option>{choices.map((v) => <option key={v.offsetSeconds} value={v.offsetSeconds}>{`UTC${v.offsetSeconds >= 0 ? "+" : "−"}${Math.floor(Math.abs(v.offsetSeconds) / 3600).toString().padStart(2, "0")}:${(Math.abs(v.offsetSeconds) % 3600 / 60).toString().padStart(2, "0")}`}</option>)}
      </select></label>}
      {selected && <p>{formatScheduledTime(selected.scheduledAt, timeZone)}</p>}
      <p>If the scheduled time is missed, confirm sending on this computer.</p>
      {info?.sharing && <p>Account, subject, schedule, and status are shared encrypted with your sync group. Recipients, message bodies, and attachments stay on this computer.</p>}
      {error && <p role="alert">{error}</p>}
      <div className="schedule-send-actions"><button type="button" className="btn" disabled={busy} onClick={onClose}>Cancel</button><button className="btn btn-primary" type="submit" disabled={busy || !selected || selected.scheduledAt <= Date.now() || !info}>{initialAt ? "Save schedule" : "Schedule email"}</button></div>
    </form>
  </Modal>;
}
