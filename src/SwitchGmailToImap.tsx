import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useId, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { GmailImapSwitchPreview } from "./domain";
import { errorMessage } from "./errors";
import { GMAIL_APP_PASSWORD_HELP } from "./ImapAccountSetup";

export function SwitchGmailToImap({ email, onCancel, onSwitched }: {
  email: string;
  onCancel(): void;
  onSwitched(): Promise<void>;
}) {
  const helpId = useId();
  const passwordInput = useRef<HTMLInputElement>(null);
  const reviewHeading = useRef<HTMLHeadingElement>(null);
  const [password, setPassword] = useState("");
  const [review, setReview] = useState<GmailImapSwitchPreview | null>(null);
  const [labels, setLabels] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const appPassword = password.replace(/\s/g, "");

  useEffect(() => {
    if (review) reviewHeading.current?.focus();
    else passwordInput.current?.focus();
  }, [review]);

  async function submit() {
    if (busy || !appPassword) return;
    setBusy(true);
    setError(null);
    try {
      if (review) {
        const choices = review.splitInboxes.map(({ id, oldLabelId }) => ({
          id, oldLabelId, labelId: labels[id] || null,
        }));
        await mailClient.switchGmailToImap(email, appPassword, choices);
        setPassword("");
        await onSwitched();
      } else {
        const preview = await mailClient.previewGmailImapSwitch(email, appPassword);
        setLabels(Object.fromEntries(preview.splitInboxes.map((split) => [split.id, split.suggestedLabelId ?? ""])));
        setReview(preview);
      }
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="imap-setup" data-shortcut-scope="modal" onSubmit={(event) => {
      event.preventDefault();
      void submit();
    }}>
      <h3 ref={reviewHeading} tabIndex={-1}>{review ? "Review Gmail switch" : "Switch to Gmail via IMAP"}</h3>
      <p>{email}</p>
      <fieldset className="imap-setup-fields" disabled={busy}>
        {review ? <>
          <p>
            ThreeStrands will keep {review.taskCount} {review.taskCount === 1 ? "task" : "tasks"}, {review.goalCount} {review.goalCount === 1 ? "goal" : "goals"}, and this account’s settings.
            Your mail will sync again from Gmail.
          </p>
          <p>This switches the connection on this device. Your other devices stay connected and can switch separately.</p>
          <p>Local drafts, Outbox items, scheduled sends, and pending mail changes for this account are cleared here. Shared snoozes and other devices’ scheduled sends are kept.</p>
          {review.splitInboxes.length > 0 && (
            <p>Choose the labels your Split Inboxes will use on this device.</p>
          )}
          {review.unverifiedTaskLinks.length > 0 && <>
            <p>These tasks will be kept. Their original mail links need review after switching.</p>
            <ul>{review.unverifiedTaskLinks.map((title, index) => <li key={index}>{title}</li>)}</ul>
          </>}
          {review.splitInboxes.map((split) => (
            <label key={split.id} className="settings-field settings-field-row">
              <span>{split.name}{split.oldLabelName ? ` · ${split.oldLabelName}` : ""}</span>
              <select value={labels[split.id] ?? ""} onChange={(event) => {
                const value = event.target.value;
                setLabels((previous) => ({ ...previous, [split.id]: value }));
              }}>
                <option value="">Keep rule without an IMAP label</option>
                {review.labels.map((label) => <option key={label.id} value={label.id}>{label.name}</option>)}
              </select>
            </label>
          ))}
          {review.splitInboxes.some((split) => !labels[split.id]) && (
            <p className="settings-hint">Choose an IMAP label for each Split Inbox you want to use after switching.</p>
          )}
        </> : <>
          <label className="settings-field settings-field-row">
            <span>App Password</span>
            <input ref={passwordInput} type="password" value={password} onChange={(event) => setPassword(event.target.value)}
              autoComplete="off" required aria-describedby={helpId} />
          </label>
          <p className="settings-hint settings-field-detail" id={helpId}>
            Create an app password for this Google Account.{" "}
            <a href={GMAIL_APP_PASSWORD_HELP} onClick={(event) => {
              event.preventDefault();
              void openUrl(GMAIL_APP_PASSWORD_HELP).catch((reason) => setError(errorMessage(reason)));
            }}>Google’s app password instructions</a>
          </p>
        </>}
      </fieldset>
      {busy && <p role="status">{review ? "Switching the connection…" : "Checking IMAP and SMTP…"}</p>}
      {error && <p role="alert">{error}</p>}
      <div className="account-card-controls">
        <button type="submit" className="btn btn-primary" disabled={busy || !appPassword}>
          {review ? "Switch to Gmail via IMAP" : "Test Connection"}
        </button>
        {review && <button type="button" className="btn" disabled={busy} onClick={() => {
          setReview(null);
          setError(null);
        }}>Back</button>}
        <button type="button" className="btn" disabled={busy} onClick={onCancel}>Cancel</button>
      </div>
    </form>
  );
}
