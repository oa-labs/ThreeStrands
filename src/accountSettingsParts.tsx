import { useState } from "react";
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
 * never a single click and the two scopes are compared side by side. The
 * broader "remove on all devices" scope carries a second "are you sure?"
 * step, since it also disconnects the member's other devices. */
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
  const [confirmingEverywhere, setConfirmingEverywhere] = useState(false);

  if (confirmingEverywhere) {
    return (
      <InlineConfirm
        ariaLabel={`Remove ${kind} account on all devices confirmation`}
        cancelLabel="Keep connected"
        onCancel={() => setConfirmingEverywhere(false)}
        disabled={disabled}
        actions={[
          { label: "Yes, remove everywhere", className: "btn-danger-strong", onClick: onRemoveEverywhere },
        ]}
      >
        <strong>Are you sure?</strong><br />
        This disconnects {email} from <strong>every device</strong> where it is signed in, not just
        this one. Nothing is deleted from {service}.
      </InlineConfirm>
    );
  }

  return (
    <InlineConfirm
      ariaLabel={`Disconnect ${kind} account confirmation`}
      cancelLabel="Cancel"
      onCancel={onCancel}
      disabled={disabled}
      actions={[
        { label: "Disconnect this device", className: "btn-danger", onClick: onDisconnect },
        { label: "Remove on all devices", className: "btn-danger-strong", onClick: () => setConfirmingEverywhere(true) },
      ]}
    >
      <strong>Disconnect {email}?</strong><br />
      This device forgets the account{kind === "mail" ? " and its local cache" : ""}; removing it on all devices also disconnects your other devices. Nothing is deleted from {service}.
    </InlineConfirm>
  );
}
