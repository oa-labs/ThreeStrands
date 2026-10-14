import { Mail, X } from "lucide-react";
import type { Account } from "./domain";
import { ICON_SIZE } from "./iconSizes";

/**
 * A one-time prompt when another device shares a mail account this device
 * has not set up. The account is only offered, never connected, so the
 * notice is informational: Review opens the account list, and either choice
 * marks these accounts as seen so the notice does not return for them.
 */
export function AvailableAccountsNotice({ accounts, suppressed, onReview, onDismiss }: {
  accounts: Account[];
  suppressed: boolean;
  onReview(): void;
  onDismiss(): void;
}) {
  if (suppressed || accounts.length === 0) return null;
  const [first] = accounts;
  const name = first.nickname?.trim() || first.displayName || first.email;
  return (
    <div className="toast available-accounts-toast" role="status" aria-live="polite">
      <Mail className="enrollment-request-icon" size={ICON_SIZE.lg} aria-hidden="true" />
      <span className="enrollment-request-message">
        {accounts.length === 1
          ? `${name} is set up on another device. You can set it up here too.`
          : `${accounts.length} accounts from your other devices can be set up here.`}
      </span>
      <button className="btn-link" onClick={onReview}>Review</button>
      <button className="btn-icon btn-icon-sm" aria-label="Dismiss" onClick={onDismiss}>
        <X size={ICON_SIZE.sm} />
      </button>
    </div>
  );
}
