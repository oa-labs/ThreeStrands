import type { SenderRule } from "./domain";
import { formatHistoryDate } from "./contactContext";
import { useSettingsOperation } from "./settingsOperations";

/** "news@example.com" or "Anyone at example.com". */
export function describeSenderRule(rule: SenderRule): string {
  return rule.matchKind === "domain" ? `Anyone at ${rule.matchValue}` : rule.matchValue;
}

/**
 * Every muted sender, by account, to review and unmute. Muting happens from
 * a bulk sender's section in the context panel, where its history is at hand.
 */
export function MutedSendersSettings({ rules, onUnmute }: {
  rules: SenderRule[];
  onUnmute(id: string): Promise<void>;
}) {
  const { pending, error, runFor } = useSettingsOperation();
  const sorted = [...rules].sort((left, right) =>
    left.accountId.localeCompare(right.accountId) || describeSenderRule(left).localeCompare(describeSenderRule(right)));
  return (
    <section className="settings-section accounts-manager" aria-label="Muted Senders">
      <div className="accounts-manager-header">
        <div>
          <h3>Muted Senders</h3>
          <p>
            New mail from these senders is marked read and archived as it arrives, so it stays
            in All Mail and search but out of your Inbox.
          </p>
        </div>
      </div>
      {sorted.length === 0 ? (
        <div className="accounts-empty">
          <strong>No muted senders</strong>
          <p>Open a newsletter or notification and choose Mute in the side panel.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {sorted.map((rule) => (
            <li className="account-card" key={rule.id}>
              <div className="account-card-row">
                <div className="account-card-identity">
                  <strong>{describeSenderRule(rule)}</strong>
                  <span className="account-card-email">{rule.accountId} · muted {formatHistoryDate(rule.createdAt)}</span>
                </div>
              </div>
              <div className="account-card-controls">
                <span className="accounts-list-actions">
                  <button
                    type="button"
                    className="btn btn-sm"
                    aria-label={`Unmute ${describeSenderRule(rule)}`}
                    disabled={pending !== null}
                    onClick={() => runFor(rule.id, () => onUnmute(rule.id))}
                  >
                    Unmute
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}
