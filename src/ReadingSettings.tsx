import {
  DEFAULT_AUTO_READ_DELAY_SECONDS, MAX_AUTO_READ_DELAY_SECONDS, MIN_AUTO_READ_DELAY_SECONDS,
  parseAfterTriage, type AfterTriage,
} from "./settings";

type OpenMode = "immediately" | "delay" | "never";

const OPEN_MODE_HINTS: Record<OpenMode, string> = {
  immediately: "Conversations are marked read as soon as you open them.",
  delay: "Conversations you leave sooner stay unread.",
  never: "Conversations stay unread until you mark them read.",
};

const AFTER_TRIAGE_OPTIONS: { value: AfterTriage; label: string }[] = [
  { value: "older", label: "Open Next Older" },
  { value: "newer", label: "Open Next Newer" },
  { value: "list", label: "Return to List" },
];

/** A delay of zero is the "Immediately" choice, so the delay field starts at one second. */
const MIN_DELAY_CHOICE_SECONDS = Math.max(1, MIN_AUTO_READ_DELAY_SECONDS);

function SettingToggle({ id, label, hint, checked, onChange }: {
  id: string;
  label: string;
  hint: string;
  checked: boolean;
  onChange(value: boolean): void;
}) {
  return (
    <div className="settings-item">
      <label className="settings-switch">
        <span>{label}</span>
        <input
          type="checkbox"
          checked={checked}
          onChange={(event) => onChange(event.target.checked)}
          aria-describedby={`${id}-hint`}
        />
      </label>
      <p id={`${id}-hint`} className="settings-hint">{hint}</p>
    </div>
  );
}

export function ReadingSettings({
  markReadOnArchive,
  onMarkReadOnArchiveChange,
  markReadOnOpen,
  onMarkReadOnOpenChange,
  markReadOnTrash,
  onMarkReadOnTrashChange,
  markReadOnSnooze,
  onMarkReadOnSnoozeChange,
  snoozeReturnsUnread,
  onSnoozeReturnsUnreadChange,
  afterTriage,
  onAfterTriageChange,
  autoReadDelaySeconds,
  onAutoReadDelayChange,
}: {
  markReadOnArchive: boolean;
  onMarkReadOnArchiveChange(value: boolean): void;
  markReadOnOpen: boolean;
  onMarkReadOnOpenChange(value: boolean): void;
  markReadOnTrash: boolean;
  onMarkReadOnTrashChange(value: boolean): void;
  markReadOnSnooze: boolean;
  onMarkReadOnSnoozeChange(value: boolean): void;
  snoozeReturnsUnread: boolean;
  onSnoozeReturnsUnreadChange(value: boolean): void;
  afterTriage: AfterTriage;
  onAfterTriageChange(value: AfterTriage): void;
  autoReadDelaySeconds: number;
  onAutoReadDelayChange(value: number): void;
}) {
  const openMode: OpenMode = !markReadOnOpen ? "never" : autoReadDelaySeconds === 0 ? "immediately" : "delay";

  const chooseOpenMode = (mode: OpenMode) => {
    if (mode === "never") {
      onMarkReadOnOpenChange(false);
      return;
    }
    if (!markReadOnOpen) onMarkReadOnOpenChange(true);
    if (mode === "immediately") onAutoReadDelayChange(0);
    else if (autoReadDelaySeconds === 0) onAutoReadDelayChange(DEFAULT_AUTO_READ_DELAY_SECONDS);
  };

  return (
    <>
      <section className="settings-section reading-settings" aria-label="Mark as Read">
        <h3>Mark as Read</h3>
        <div className="settings-item">
          <div className="settings-item-control">
            <label htmlFor="mark-read-on-open">Mark Read After Opening</label>
            <div className="settings-row">
              <select
                id="mark-read-on-open"
                value={openMode}
                onChange={(event) => chooseOpenMode(event.target.value as OpenMode)}
                aria-describedby="mark-read-on-open-hint"
              >
                <option value="immediately">Immediately</option>
                <option value="delay">After a Delay</option>
                <option value="never">Never</option>
              </select>
              {openMode === "delay" ? (
                <>
                  <input
                    type="number"
                    min={MIN_DELAY_CHOICE_SECONDS}
                    max={MAX_AUTO_READ_DELAY_SECONDS}
                    step="1"
                    value={autoReadDelaySeconds}
                    aria-label="Mark Read Delay in Seconds"
                    aria-describedby="mark-read-on-open-hint"
                    onChange={(event) => {
                      if (event.target.value === "") return;
                      const next = Number(event.target.value);
                      // Zero belongs to the Immediately choice; ignoring it here
                      // keeps the field from vanishing mid-edit.
                      if (Number.isFinite(next) && next >= MIN_DELAY_CHOICE_SECONDS) onAutoReadDelayChange(next);
                    }}
                  />
                  <span aria-hidden="true">sec</span>
                </>
              ) : null}
            </div>
          </div>
          <p id="mark-read-on-open-hint" className="settings-hint">{OPEN_MODE_HINTS[openMode]}</p>
        </div>
        <SettingToggle
          id="archive-mark-read"
          label="Mark Read When Archiving"
          hint="Includes conversations you archive from the list without opening them."
          checked={markReadOnArchive}
          onChange={onMarkReadOnArchiveChange}
        />
        <SettingToggle
          id="trash-mark-read"
          label="Mark Read When Moving to Trash"
          hint="Undo restores each conversation's earlier read status."
          checked={markReadOnTrash}
          onChange={onMarkReadOnTrashChange}
        />
        <SettingToggle
          id="snooze-mark-read"
          label="Mark Read When Snoozing"
          hint="Undo restores each conversation's earlier read status."
          checked={markReadOnSnooze}
          onChange={onMarkReadOnSnoozeChange}
        />
      </section>
      <section className="settings-section reading-settings" aria-label="Triage">
        <h3>Triage</h3>
        <div className="settings-item">
          <div className="settings-item-control">
            <label htmlFor="after-triage">After Archiving or Moving to Trash</label>
            <select
              id="after-triage"
              value={afterTriage}
              onChange={(event) => onAfterTriageChange(parseAfterTriage(event.target.value))}
              aria-describedby="after-triage-hint"
            >
              {AFTER_TRIAGE_OPTIONS.map((option) => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </select>
          </div>
          <p id="after-triage-hint" className="settings-hint">
            Chooses what opens in place of the conversation you just handled. Also applies to Spam.
          </p>
        </div>
        <SettingToggle
          id="snooze-returns-unread"
          label="Return Snoozed Conversations as Unread"
          hint="Snoozed conversations come back to the Inbox marked unread, even if you had read them."
          checked={snoozeReturnsUnread}
          onChange={onSnoozeReturnsUnreadChange}
        />
      </section>
    </>
  );
}
