import { useEffect, useId, useState } from "react";
import { Plus, X } from "lucide-react";
import { mailClient } from "./data/client";
import type { ImapSendingOptions, SendingIdentity, SendingIdentityInput } from "./domain";
import { useSettingsOperation } from "./settingsOperations";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";

/** One editable secondary-identity row (address + optional display name). */
type IdentityRow = { address: string; displayName: string; replyTo: string; isDefault: boolean };

function editableAddresses(identities: SendingIdentity[]): IdentityRow[] {
  return identities.slice(1).map((identity) => ({
    address: identity.address,
    displayName: identity.displayName ?? "",
    replyTo: identity.replyTo ?? "",
    isDefault: Boolean(identity.isDefault),
  }));
}

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
  const { pending, busy, error, errorKey, runFor } = useSettingsOperation();
  const [options, setOptions] = useState<ImapSendingOptions | null>(null);
  const [rows, setRows] = useState<IdentityRow[]>([]);
  const [primaryReplyTo, setPrimaryReplyTo] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [addressesSaved, setAddressesSaved] = useState(false);
  const [sentModeSaved, setSentModeSaved] = useState(false);
  const headingId = useId();
  const replyToId = useId();

  useEffect(() => {
    let active = true;
    setOptions(null);
    setLoadError(null);
    setAddressesSaved(false);
    setSentModeSaved(false);
    void mailClient.getImapSendingOptions(account).then((next) => {
      if (!active) return;
      setOptions(next);
      setPrimaryReplyTo(next.identities[0]?.replyTo ?? "");
      setRows(editableAddresses(next.identities));
    }).catch((reason: unknown) => {
      if (active) setLoadError(errorMessage(reason));
    });
    return () => { active = false; };
  }, [account]);

  const heading = <h4 id={headingId}>Sending addresses</h4>;
  if (loadError || !options) {
    return (
      <section className="settings-subsection" aria-labelledby={headingId}>
        {heading}
        {loadError
          ? <p className="form-error" role="alert">{loadError}</p>
          : <p role="status">Loading…</p>}
      </section>
    );
  }

  const signInAddress = options.identities[0]?.address ?? account;

  const updateRow = (index: number, patch: Partial<IdentityRow>) =>
    setRows((current) => current.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  const addRow = () => setRows((current) => [...current, { address: "", displayName: "", replyTo: "", isDefault: false }]);
  const removeRow = (index: number) => setRows((current) => current.filter((_, i) => i !== index));
  const defaultRow = rows.findIndex((row) => row.isDefault && row.address.trim());
  const secondaryAddresses: SendingIdentityInput[] = rows
    .filter((row) => row.address.trim())
    .map((row) => ({
      address: row.address.trim(),
      displayName: row.displayName.trim() || null,
      replyTo: row.replyTo.trim() || undefined,
      isDefault: row.isDefault || undefined,
    }));
  const savedSecondaryAddresses = options.identities.slice(1);
  const addressesChanged = primaryReplyTo.trim() !== (options.identities[0]?.replyTo ?? "")
    || secondaryAddresses.length !== savedSecondaryAddresses.length
    || secondaryAddresses.some((address, index) => {
      const saved = savedSecondaryAddresses[index];
      return address.address !== saved.address
        || address.displayName !== (saved.displayName || null)
        || address.replyTo !== (saved.replyTo || undefined)
        || Boolean(address.isDefault) !== Boolean(saved.isDefault);
    });

  const saveIdentities = () =>
    runFor("identities", async () => {
      setAddressesSaved(false);
      const identities = await mailClient.setImapIdentities(account, [
        {
          address: signInAddress,
          displayName: options.identities[0]?.displayName ?? null,
          replyTo: primaryReplyTo.trim() || undefined,
        },
        ...secondaryAddresses,
      ]);
      setOptions({ ...options, identities });
      setPrimaryReplyTo(identities[0]?.replyTo ?? "");
      setRows(editableAddresses(identities));
      setAddressesSaved(true);
    });

  const setServerSavesSent = (enabled: boolean) =>
    runFor("sentCopy", async () => {
      setSentModeSaved(false);
      await mailClient.setImapServerSavesSent(account, enabled);
      setOptions({ ...options, serverSavesSentOverride: enabled });
      setSentModeSaved(true);
    });

  const resetDetection = () =>
    runFor("resetDetection", async () => {
      await mailClient.resetImapSentCopyDetection(account);
      setOptions({ ...options, sentCopyDetected: null });
    });

  const automatic = !options.serverSavesSentOverride;

  return (
    <>
      <section className="settings-subsection" aria-labelledby={headingId}>
        {heading}
        <p className="imap-identity-signin">
          <span className="settings-hint">Sign-in address</span>
          <span className="imap-identity-signin-address">{signInAddress}</span>
        </p>
        <div className="settings-field imap-identity-reply-to">
          <label htmlFor={replyToId}>Reply-To (optional)</label>
          <input id={replyToId} type="email" value={primaryReplyTo} disabled={busy} aria-describedby={`${replyToId}-hint`}
            onChange={(event) => setPrimaryReplyTo(event.target.value)} />
          <p id={`${replyToId}-hint`} className="settings-hint">Leave blank to receive replies at your sign-in address.</p>
        </div>
        {rows.length > 0 && (
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
        )}
        {secondaryAddresses.length > 0 && (
          <label className="settings-field settings-field-row">
            <span>Default From address</span>
            <select value={defaultRow} disabled={busy} onChange={(event) => {
              const selected = Number(event.target.value);
              setRows((current) => current.map((row, index) => ({ ...row, isDefault: index === selected })));
            }}>
              <option value={-1}>{signInAddress}</option>
              {rows.map((row, index) => row.address.trim() && (
                <option key={index} value={index}>{row.address.trim()}</option>
              ))}
            </select>
          </label>
        )}
        <div className="imap-sending-actions">
          <button type="button" className="btn btn-sm" disabled={busy} onClick={addRow}>
            <Plus size={ICON_SIZE.sm} /> Add address
          </button>
          <button type="button" className="btn btn-sm btn-primary" disabled={busy || !addressesChanged} onClick={saveIdentities}>
            {pending === "identities" ? "Saving…" : "Save addresses"}
          </button>
          {addressesSaved && !addressesChanged && <p className="settings-hint" role="status">Addresses saved.</p>}
        </div>
        {error && errorKey === "identities" && <p className="form-error" role="alert">{error}</p>}
      </section>

      <div className="settings-subsection">
        <fieldset className="imap-sent-copy" aria-describedby={`${headingId}-sent-copy-intro`}>
          <legend>Save sent messages</legend>
          <p id={`${headingId}-sent-copy-intro`} className="settings-hint">
            Choose who saves a copy in this account&apos;s Sent folder.
          </p>
          <SentCopyOption
            name={`sent-copy-${account}`}
            checked={automatic}
            disabled={busy}
            label="Automatic (recommended)"
            description="ThreeStrands saves a copy when your server doesn't."
            onSelect={() => setServerSavesSent(false)}
          />
          <SentCopyOption
            name={`sent-copy-${account}`}
            checked={!automatic}
            disabled={busy}
            label="Use the server’s copy"
            description="Choose this if your server saves sent messages. Otherwise, sent messages won't appear in your Sent folder."
            onSelect={() => setServerSavesSent(true)}
          />
          {automatic ? (
            <div className="imap-sent-copy-status">
              <p className="settings-hint" role="status">{detectionStatus(options.sentCopyDetected)}</p>
              {options.sentCopyDetected !== null && (
                <button type="button" className="btn btn-sm btn-ghost" disabled={busy} onClick={resetDetection}>
                  {pending === "resetDetection" ? "Resetting…" : "Re-check on next send"}
                </button>
              )}
            </div>
          ) : sentModeSaved && <p className="settings-hint" role="status">Sent settings saved.</p>}
          {error && errorKey !== "identities" && <p className="form-error" role="alert">{error}</p>}
        </fieldset>
      </div>
    </>
  );
}

/** What Automatic mode has learned about the server, in plain words. */
function detectionStatus(detected: boolean | null): string {
  if (detected === null) return "ThreeStrands will check on your next send.";
  return detected
    ? "Your mail server saves a copy in Sent."
    : "ThreeStrands saves a copy in Sent.";
}

/** One stacked radio choice with its own explanation underneath. */
function SentCopyOption({
  name,
  checked,
  disabled,
  label,
  description,
  onSelect,
}: {
  name: string;
  checked: boolean;
  disabled: boolean;
  label: string;
  description: string;
  onSelect(): void;
}) {
  const id = useId();
  return (
    <div className="imap-sent-copy-option">
      <input
        id={id}
        type="radio"
        name={name}
        checked={checked}
        disabled={disabled}
        aria-describedby={`${id}-description`}
        onChange={onSelect}
      />
      <label htmlFor={id}>{label}</label>
      <p id={`${id}-description`} className="settings-hint">{description}</p>
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
  const replyToId = useId();
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
        <label htmlFor={nameId}>Name (Optional)</label>
        <input
          id={nameId}
          value={row.displayName}
          disabled={disabled}
          placeholder="Display name"
          onChange={(event) => onChange({ displayName: event.target.value })}
        />
        <label htmlFor={replyToId}>Reply-To {index + 1}</label>
        <input
          id={replyToId}
          type="email"
          value={row.replyTo}
          disabled={disabled}
          onChange={(event) => onChange({ replyTo: event.target.value })}
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
