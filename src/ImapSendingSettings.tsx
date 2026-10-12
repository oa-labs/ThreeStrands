import { useCallback, useEffect, useId, useState } from "react";
import { Plus, X } from "lucide-react";
import { mailClient } from "./data/client";
import type { ImapSendingOptions, SendingIdentityInput } from "./domain";
import { useSettingsOperation } from "./settingsOperations";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";

/** One editable secondary-identity row (address + optional display name). */
type IdentityRow = { address: string; displayName: string };

/**
 * The "Sending" panel for ONE IMAP account (rendered only when
 * `account.provider === "imap"`): the ordered sending addresses (sign-in
 * address first and read-only, then an editable list) and the "Sent mail
 * copy" behaviour. Nothing here applies to Gmail, and there are no password
 * fields. Backend validation messages are shown inline; the numeric address
 * limit is enforced by the backend and surfaced as its message, never a
 * second hard-coded copy here.
 */
export function ImapSendingSettings({ account }: { account: string }) {
  const { pending, busy, error, actFor } = useSettingsOperation();
  const [options, setOptions] = useState<ImapSendingOptions | null>(null);
  const [rows, setRows] = useState<IdentityRow[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const headingId = useId();

  const load = useCallback(async () => {
    const next = await mailClient.getImapSendingOptions(account);
    setOptions(next);
    // The first identity is the sign-in address (read-only); the rest are the
    // editable secondary addresses.
    setRows(next.identities.slice(1).map((identity) => ({ address: identity.address, displayName: identity.displayName ?? "" })));
  }, [account]);

  useEffect(() => {
    setLoadError(null);
    void load().catch((reason) => setLoadError(errorMessage(reason)));
  }, [load]);

  if (loadError) {
    return (
      <section className="settings-subsection imap-sending" aria-labelledby={headingId}>
        <h4 id={headingId}>Sending</h4>
        <p className="form-error" role="alert">{loadError}</p>
      </section>
    );
  }
  if (!options) {
    return (
      <section className="settings-subsection imap-sending" aria-labelledby={headingId}>
        <h4 id={headingId}>Sending</h4>
        <p role="status">Loading…</p>
      </section>
    );
  }

  const signInAddress = options.identities[0]?.address ?? account;

  const updateRow = (index: number, patch: Partial<IdentityRow>) =>
    setRows((current) => current.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  const addRow = () => setRows((current) => [...current, { address: "", displayName: "" }]);
  const removeRow = (index: number) => setRows((current) => current.filter((_, i) => i !== index));

  const saveIdentities = () =>
    actFor("identities", async () => {
      const submitted: SendingIdentityInput[] = [
        // The sign-in address is always first and cannot be removed; keep its
        // (read-only) display name if the server reported one.
        { address: signInAddress, displayName: options.identities[0]?.displayName ?? null },
        ...rows
          .filter((row) => row.address.trim())
          .map((row) => ({ address: row.address.trim(), displayName: row.displayName.trim() || null })),
      ];
      await mailClient.setImapIdentities(account, submitted);
      await load();
    });

  const setServerSavesSent = (enabled: boolean) =>
    actFor("sentCopy", async () => {
      await mailClient.setImapServerSavesSent(account, enabled);
      await load();
    });

  const resetDetection = () =>
    actFor("resetDetection", async () => {
      await mailClient.resetImapSentCopyDetection(account);
      await load();
    });

  const detectionStatus =
    options.sentCopyDetected === null
      ? "ThreeStrands will check on your next message."
      : options.sentCopyDetected
        ? "Your server saves a copy itself."
        : "ThreeStrands saves a copy for you.";

  return (
    <section className="settings-subsection imap-sending" aria-labelledby={headingId}>
      <h4 id={headingId}>Sending</h4>

      <div className="imap-sending-addresses">
        <h5>Sending addresses</h5>
        <p>Addresses this account can send mail from. Your sign-in address is always available.</p>
        <SignInAddressRow address={signInAddress} />
        <ul className="imap-identity-list">
          {rows.map((row, index) => (
            <IdentityRowFields
              key={index}
              index={index}
              row={row}
              disabled={busy}
              onChange={(patch) => updateRow(index, patch)}
              onRemove={() => removeRow(index)}
            />
          ))}
        </ul>
        <div className="imap-sending-actions">
          <button type="button" className="btn btn-sm" disabled={busy} onClick={addRow}>
            <Plus size={ICON_SIZE.sm} /> Add address
          </button>
          <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => void saveIdentities()}>
            {pending === "identities" ? "Saving…" : "Save addresses"}
          </button>
        </div>
      </div>

      <fieldset className="imap-sent-copy">
        <legend>Sent mail copy</legend>
        <p>If this is wrong, your Sent folder could show the message twice or not at all.</p>
        <label className="imap-sent-copy-option">
          <input
            type="radio"
            name={`sent-copy-${account}`}
            checked={!options.serverSavesSentOverride}
            disabled={busy}
            onChange={() => void setServerSavesSent(false)}
          />
          <span>Automatic (recommended)</span>
        </label>
        <label className="imap-sent-copy-option">
          <input
            type="radio"
            name={`sent-copy-${account}`}
            checked={options.serverSavesSentOverride}
            disabled={busy}
            onChange={() => void setServerSavesSent(true)}
          />
          <span>My mail server saves sent mail itself — don&apos;t save a copy</span>
        </label>
        <p className="imap-sent-copy-status" role="status">{detectionStatus}</p>
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void resetDetection()}>
          {pending === "resetDetection" ? "Checking…" : "Check again"}
        </button>
      </fieldset>

      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

function SignInAddressRow({ address }: { address: string }) {
  const id = useId();
  return (
    <div className="imap-identity-row imap-identity-signin">
      <label htmlFor={id}>Sign-in address</label>
      <input id={id} value={address} readOnly aria-readonly="true" />
    </div>
  );
}

function IdentityRowFields({
  index,
  row,
  disabled,
  onChange,
  onRemove,
}: {
  index: number;
  row: IdentityRow;
  disabled: boolean;
  onChange(patch: Partial<IdentityRow>): void;
  onRemove(): void;
}) {
  const addressId = useId();
  const nameId = useId();
  return (
    <li className="imap-identity-row">
      <div className="imap-identity-fields">
        <label htmlFor={addressId}>Address {index + 1}</label>
        <input
          id={addressId}
          type="email"
          value={row.address}
          disabled={disabled}
          placeholder="alias@example.com"
          onChange={(event) => onChange({ address: event.target.value })}
        />
        <label htmlFor={nameId}>Name (optional)</label>
        <input
          id={nameId}
          value={row.displayName}
          disabled={disabled}
          placeholder="Display name"
          onChange={(event) => onChange({ displayName: event.target.value })}
        />
      </div>
      <button
        type="button"
        className="btn-icon btn-icon-sm"
        aria-label={`Remove address ${index + 1}`}
        disabled={disabled}
        onClick={onRemove}
      >
        <X size={ICON_SIZE.sm} />
      </button>
    </li>
  );
}
