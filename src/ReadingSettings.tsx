import { MAX_AUTO_READ_DELAY_SECONDS, MIN_AUTO_READ_DELAY_SECONDS } from "./settings";

export function ReadingSettings({
  markReadOnArchive,
  onMarkReadOnArchiveChange,
  autoReadDelaySeconds,
  onAutoReadDelayChange,
}: {
  markReadOnArchive: boolean;
  onMarkReadOnArchiveChange(value: boolean): void;
  autoReadDelaySeconds: number;
  onAutoReadDelayChange(value: number): void;
}) {
  return (
    <section className="settings-section" aria-label="Mark as Read">
      <h3>Mark as Read</h3>
      <label className="settings-field settings-field-inline">
        <span>After Opening a Conversation</span>
        <div className="settings-row">
          <input
            type="number"
            min={MIN_AUTO_READ_DELAY_SECONDS}
            max={MAX_AUTO_READ_DELAY_SECONDS}
            step="1"
            value={autoReadDelaySeconds}
            aria-label="Auto-Read Delay"
            onChange={(event) => {
              if (event.target.value === "") return;
              const next = Number(event.target.value);
              if (Number.isFinite(next)) onAutoReadDelayChange(next);
            }}
          />
          <span>seconds</span>
        </div>
      </label>
      <p className="settings-hint">
        Set to 0 to mark conversations read immediately. The timer resets when you open a different conversation.
      </p>
      <label className="settings-switch">
        <span>When Archiving a Conversation</span>
        <input
          type="checkbox"
          checked={markReadOnArchive}
          onChange={(event) => onMarkReadOnArchiveChange(event.target.checked)}
          aria-describedby="archive-mark-read-hint"
        />
      </label>
      <p id="archive-mark-read-hint" className="settings-hint">
        Also applies when archiving without opening the conversation.
      </p>
    </section>
  );
}
