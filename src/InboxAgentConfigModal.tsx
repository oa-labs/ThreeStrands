import { useRef, useState } from "react";
import { Modal } from "./AppChrome";
import type { Account, InboxAgentConfig, UpdateInboxAgentConfig } from "./domain";

const SCHEDULE_OPTIONS: { value: string; label: string; hours: number | null }[] = [
  { value: "manual", label: "Manual Only", hours: null },
  { value: "daily", label: "Daily", hours: 24 },
  { value: "weekly", label: "Weekly", hours: 168 },
];

export function InboxAgentConfigModal({
  config,
  accounts,
  busy,
  onSave,
  onClose,
}: {
  config: InboxAgentConfig;
  accounts: readonly Account[];
  busy: boolean;
  onSave(next: UpdateInboxAgentConfig): Promise<boolean>;
  onClose(): void;
}) {
  const ageInputRef = useRef<HTMLInputElement>(null);
  const [enabled, setEnabled] = useState(config.enabled);
  const [ageDays, setAgeDays] = useState(String(config.ageDays));
  const [maxThreads, setMaxThreads] = useState(String(config.maxThreadsPerRun));
  const [includeStarred, setIncludeStarred] = useState(config.includeStarred);
  const [accountId, setAccountId] = useState(config.accountId ?? "");
  const [schedule, setSchedule] = useState(
    config.scheduleHours === 24 ? "daily" : config.scheduleHours === 168 ? "weekly" : "manual",
  );
  const [validationError, setValidationError] = useState("");

  async function handleSubmit(event: React.FormEvent) {
    event.preventDefault();
    const parsedAge = Number.parseInt(ageDays, 10);
    const parsedMax = Number.parseInt(maxThreads, 10);
    if (!Number.isFinite(parsedAge) || parsedAge < 1 || parsedAge > 365) {
      setValidationError("Minimum email age must be between 1 and 365 days.");
      return;
    }
    if (!Number.isFinite(parsedMax) || parsedMax < 1 || parsedMax > 30) {
      setValidationError("Max emails per run must be between 1 and 30.");
      return;
    }
    setValidationError("");
    const scheduleHours = SCHEDULE_OPTIONS.find((opt) => opt.value === schedule)?.hours ?? null;
    const ok = await onSave({
      agentId: config.agentId,
      enabled,
      ageDays: parsedAge,
      maxThreadsPerRun: parsedMax,
      includeStarred,
      accountId: accountId || null,
      scheduleHours,
    });
    if (ok) onClose();
  }

  return (
    <Modal
      title={`Configure ${config.name}`}
      className="assistant-config-modal"
      shortcutScope="modal"
      initialFocusRef={ageInputRef}
      dismissible={!busy}
      onClose={onClose}
    >
      <form
        className="modal-form"
        noValidate
        onSubmit={(e) => { void handleSubmit(e); }}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            void handleSubmit(e);
          }
        }}
      >
        <p className="modal-form-hint">{config.description}</p>
        <label className="modal-form-check">
          <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
          <span>Enable This Assistant</span>
        </label>

        <div className="modal-form-row">
          <label>
            Only Review Emails Older Than (Days)
            <input
              ref={ageInputRef}
              type="number"
              min={1}
              max={365}
              value={ageDays}
              onChange={(e) => setAgeDays(e.target.value)}
            />
          </label>
          <label>
            Max Emails per Run (1–30)
            <input
              type="number"
              min={1}
              max={30}
              value={maxThreads}
              onChange={(e) => setMaxThreads(e.target.value)}
            />
          </label>
        </div>

        <label>
          Mail Account Scope
          <select value={accountId} onChange={(e) => setAccountId(e.target.value)}>
            <option value="">All Connected Accounts</option>
            {accounts
              .filter((a) => a.onThisDevice !== false)
              .map((a) => (
                <option key={a.email} value={a.email}>
                  {a.displayName ? `${a.displayName} (${a.email})` : a.email}
                </option>
              ))}
          </select>
        </label>

        <label>
          Run Schedule
          <select value={schedule} onChange={(e) => setSchedule(e.target.value)}>
            {SCHEDULE_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>{opt.label}</option>
            ))}
          </select>
        </label>

        <label className="modal-form-check">
          <input
            type="checkbox"
            checked={includeStarred}
            onChange={(e) => setIncludeStarred(e.target.checked)}
          />
          <span>Include Starred Conversations</span>
        </label>

        {validationError ? <p className="form-error" role="alert">{validationError}</p> : null}

        <div className="modal-form-actions">
          <button type="button" className="btn" disabled={busy} onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={busy}>Save</button>
        </div>
      </form>
    </Modal>
  );
}
