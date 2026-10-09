import { CircleAlert, CircleCheck } from "lucide-react";
import { InlineConfirm } from "./InlineConfirm";
import type { Account, CalendarAccount } from "./domain";
import { ICON_SIZE } from "./iconSizes";

export function AccountStatusBadge({ status }: { status: Account["status"] | CalendarAccount["status"] }) {
  return (
    <span className={`account-status ${status}`}>
      {status === "needs_reauth" ? <CircleAlert size={ICON_SIZE.xs} /> : <CircleCheck size={ICON_SIZE.xs} />}
      {status === "needs_reauth" ? "Needs reconnect" : "Connected"}
    </span>
  );
}

/** One confirmation for both removal scopes, so the destructive action is
 * never a single click and the two scopes are compared side by side. */
export function AccountDisconnectConfirm({
  kind,
  email,
  disabled,
  onCancel,
  onDisconnect,
  onRemoveEverywhere,
}: {
  kind: "mail" | "calendar";
  email: string;
  disabled: boolean;
  onCancel(): void;
  onDisconnect(): void;
  onRemoveEverywhere(): void;
}) {
  const service = kind === "mail" ? "Gmail" : "Google Calendar";
  return (
    <InlineConfirm
      ariaLabel={`Disconnect ${kind} account confirmation`}
      cancelLabel="Cancel"
      onCancel={onCancel}
      disabled={disabled}
      actions={[
        { label: "Disconnect this device", className: "btn-danger", onClick: onDisconnect },
        { label: "Remove on all devices", className: "btn-danger", onClick: onRemoveEverywhere },
      ]}
    >
      <strong>Disconnect {email}?</strong><br />
      This device forgets the account{kind === "mail" ? " and its local cache" : ""}; removing it on all devices also disconnects your other devices. {service} itself is not changed.
    </InlineConfirm>
  );
}
