import type { SenderRule } from "./domain";
import { formatHistoryDate } from "./contactContext";
import { useSettingsOperation } from "./settingsOperations";

/**
 * Every sender Mark Spam blocked, by account, with when their mail last
 * arrived, to review and unblock. Blocks whose senders go quiet are pruned
 * by the app, so this list stays short on its own.
 */
export function BlockedSendersSettings({ rules, onUnblock }: {
  rules: SenderRule[];
  onUnblock(id: string): Promise<void>;
}) {
  const { pending, error, runFor } = useSettingsOperation();
  const sorted = rules
    .filter((rule) => rule.action === "block")
    .sort((left, right) => left.accountId.localeCompare(right.accountId) || left.matchValue.localeCompare(right.matchValue));
  return (
    <section className="settings-section accounts-manager" aria-label="Blocked Senders">
      <div className="accounts-manager-header">
        <div>
          <h3>Blocked Senders</h3>
          <p className="settings-hint">
            Mark Spam blocks the sender, so their new mail goes straight to Spam. A blocked
            sender who sends nothing for six months is forgotten.
          </p>
        </div>
      </div>
      {sorted.length === 0 ? (
        <div className="accounts-empty">
          <strong>No blocked senders</strong>
          <p>Choose Mark Spam on a conversation to block its sender.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {sorted.map((rule) => (
            <li className="account-card" key={rule.id}>
              <div className="account-card-row">
                <div className="account-card-identity">
                  <strong>{rule.matchValue}</strong>
                  <span className="account-card-email">
                    {rule.accountId} · {rule.lastMatchedAt ? `Last email ${formatHistoryDate(rule.lastMatchedAt)}` : "No email since blocking"}
                  </span>
                </div>
              </div>
              <div className="account-card-controls">
                <span className="accounts-list-actions">
                  <button
                    type="button"
                    className="btn btn-sm"
                    aria-label={`Unblock ${rule.matchValue}`}
                    disabled={pending !== null}
                    onClick={() => runFor(rule.id, () => onUnblock(rule.id))}
                  >
                    Unblock
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
