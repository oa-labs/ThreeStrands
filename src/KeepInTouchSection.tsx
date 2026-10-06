import { useEffect, useState } from "react";
import { AlarmClock, CheckCircle2, LoaderCircle } from "lucide-react";
import { mailClient } from "./data/client";
import type { ContactProfile } from "./domain";
import { errorMessage } from "./errors";
import {
  KEEP_IN_TOUCH_FREQUENCIES, KEEP_IN_TOUCH_SNOOZES, MAX_KEEP_IN_TOUCH_DAYS, dateInputValue, describeDue, formatKeepInTouchDate,
  isSnoozeActive, lastTouchAt, parseIntervalDays, snoozeUntilDate, snoozeUntilDays,
} from "./keepInTouch";

type FrequencyChoice = "off" | "custom" | `${number}`;

const choiceFor = (days: number | null): FrequencyChoice =>
  days === null ? "off" : KEEP_IN_TOUCH_FREQUENCIES.some((item) => item.days === days) ? `${days}` : "custom";

/**
 * Keep-in-touch reminders for one contact. Every control applies at once
 * through its own command, separately from the profile form's Save, so an
 * unsaved form edit never travels with a reminder change or the reverse.
 */
export function KeepInTouchSection({ profile, onChanged }: { profile: ContactProfile; onChanged(profile: ContactProfile): void }) {
  const interval = profile.keepInTouch.intervalDays;
  const [choice, setChoice] = useState<FrequencyChoice>(choiceFor(interval));
  const [customDays, setCustomDays] = useState(interval === null ? "" : String(interval));
  const [snoozeDate, setSnoozeDate] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setChoice(choiceFor(interval));
    setCustomDays(interval === null ? "" : String(interval));
    setError(null);
  }, [profile.id, interval]);

  const run = async (action: () => Promise<ContactProfile>) => {
    setBusy(true);
    setError(null);
    try {
      onChanged(await action());
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };
  const applyInterval = (days: number | null) => run(async () => (await mailClient.setKeepInTouch([profile.id], days))[0]);
  const chooseFrequency = (value: FrequencyChoice) => {
    setChoice(value);
    if (value === "custom") return;
    void applyInterval(value === "off" ? null : Number(value));
  };
  const applyCustom = () => {
    const days = parseIntervalDays(customDays);
    if (days === null) {
      setError(`Enter a whole number of days from 1 to ${MAX_KEEP_IN_TOUCH_DAYS}`);
      return;
    }
    void applyInterval(days);
  };
  const snoozeToDate = () => {
    const until = snoozeUntilDate(snoozeDate);
    if (!until) {
      setError("Choose a date after today");
      return;
    }
    void run(() => mailClient.snoozeKeepInTouch(profile.id, until));
  };

  const touched = lastTouchAt(profile);
  const tomorrow = new Date();
  tomorrow.setDate(tomorrow.getDate() + 1);
  return <section className="contact-keep-in-touch" aria-label="Keep in Touch">
    <header><h2>Keep in Touch</h2>{busy ? <LoaderCircle className="spin" size={14} aria-label="Saving" /> : null}</header>
    <div className="contact-kit-controls">
      <label>Frequency
        <select value={choice} disabled={busy} onChange={(event) => chooseFrequency(event.target.value as FrequencyChoice)}>
          <option value="off">Off</option>
          {KEEP_IN_TOUCH_FREQUENCIES.map((item) => <option key={item.days} value={item.days}>{item.label}</option>)}
          <option value="custom">Custom…</option>
        </select>
      </label>
      {choice === "custom" ? <>
        <label>Every N Days
          <input type="number" inputMode="numeric" min={1} max={MAX_KEEP_IN_TOUCH_DAYS} value={customDays} disabled={busy}
            onChange={(event) => setCustomDays(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); applyCustom(); } }} />
        </label>
        <button type="button" className="contact-primary-button" disabled={busy} onClick={applyCustom}>Set Frequency</button>
      </> : null}
    </div>
    {interval !== null && profile.keepInTouchDueAt ? <>
      <p className="contact-kit-status" role="status">
        {isSnoozeActive(profile) ? `Snoozed until ${formatKeepInTouchDate(profile.keepInTouchDueAt)}` : describeDue(profile.keepInTouchDueAt)}
        {" · "}{touched ? `Last contact ${formatKeepInTouchDate(touched)}` : "No contact yet"}
      </p>
      <div className="contact-kit-actions">
        <button type="button" disabled={busy} title="Log a call, meeting, or message outside email" onClick={() => void run(() => mailClient.markContacted(profile.id))}><CheckCircle2 size={15} />Mark Contacted</button>
        <details className="contact-kit-snooze">
          <summary><AlarmClock size={15} />Snooze</summary>
          <div className="contact-kit-snooze-menu">
            {KEEP_IN_TOUCH_SNOOZES.map((item) => <button key={item.days} type="button" disabled={busy} onClick={() => void run(() => mailClient.snoozeKeepInTouch(profile.id, snoozeUntilDays(item.days)))}>{item.label}</button>)}
            <label>Until Date<input type="date" min={dateInputValue(tomorrow)} value={snoozeDate} disabled={busy} onChange={(event) => setSnoozeDate(event.target.value)} /></label>
            <button type="button" disabled={busy || !snoozeDate} onClick={snoozeToDate}>Snooze Until Date</button>
          </div>
        </details>
        {isSnoozeActive(profile) ? <button type="button" disabled={busy} onClick={() => void run(() => mailClient.snoozeKeepInTouch(profile.id, null))}>End Snooze</button> : null}
      </div>
    </> : <p className="contact-kit-hint">Choose how often you want to be in touch. Email either way counts, and so does Mark Contacted.</p>}
    {error ? <p className="contacts-error" role="alert">{error}</p> : null}
  </section>;
}
