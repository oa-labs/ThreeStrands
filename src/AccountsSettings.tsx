import {
  CircleAlert,
  ChevronDown,
  ChevronUp,
  Mail,
  Plus,
} from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { Account, AuthStatus } from "./domain";
import { formatTimeOnly } from "./threadPresentation";
import { moveItem, useSettingsOperation } from "./settingsOperations";
import { errorMessage, logBackgroundFailure } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { ImapAccountSetup } from "./ImapAccountSetup";
import { AccountStatusBadge, AccountDisconnectConfirm } from "./accountSettingsParts";

export function AccountsSettings({
  authStatus,
  accounts,
  onAdd,
  onRemove,
  onRemoveEverywhere,
  onReconnect,
  onSetDisplayName,
  onSetColor,
  onReorder,
  onImapConnected,
}: {
  authStatus: AuthStatus | null;
  accounts: Account[];
  onAdd(): Promise<void>;
  onRemove(email: string): Promise<void>;
  onRemoveEverywhere(email: string): Promise<void>;
  onReconnect(email: string): Promise<void>;
  onSetDisplayName(email: string, displayName: string | null): Promise<void>;
  onSetColor(email: string, color: string): Promise<void>;
  onReorder(emails: string[]): Promise<void>;
  onImapConnected?(): Promise<void>;
}) {
  const { pending: busyEmail, error, setError, runFor } = useSettingsOperation();
  const [accountSetup, setAccountSetup] = useState<"provider" | "imap" | null>(null);
  const accountSetupId = useId();
  const addButton = useRef<HTMLButtonElement>(null);
  const restoreAddFocus = useRef(false);
  const [imapSaved, setImapSaved] = useState(false);
  const [confirmEmail, setConfirmEmail] = useState<string | null>(null);

  useEffect(() => {
    if (restoreAddFocus.current && busyEmail === null) {
      restoreAddFocus.current = false;
      addButton.current?.focus();
    }
  }, [busyEmail]);

  const move = (index: number, direction: -1 | 1) => {
    const account = accounts[index];
    const next = moveItem(accounts, index, direction);
    if (account && next) runFor(account.email, () => onReorder(next.map((item) => item.email)));
  };

  return (
    <section className="settings-section accounts-manager" aria-label="Mail Accounts">
      <div className="accounts-manager-header">
        <div>
          <h3>Connected Mail Accounts</h3>
          <p>
            ThreeStrands keeps accounts separate and merges their inboxes by default.
            Use the sidebar or command palette to filter to one account.
          </p>
        </div>
        <button
          ref={addButton}
          type="button"
          className="btn btn-primary settings-add-account"
          disabled={busyEmail !== null}
          aria-expanded={accountSetup !== null}
          aria-controls={accountSetupId}
          onClick={() => {
            setAccountSetup(accountSetup === null ? "provider" : null);
            setImapSaved(false);
          }}
        >
          <Plus size={ICON_SIZE.md} />
          {busyEmail === "__add__" ? "Waiting for Google…" : "Add Account"}
        </button>
      </div>
      <div id={accountSetupId} className="account-setup-panel" hidden={accountSetup === null}>
        {accountSetup === "provider" && (
          <div className="account-provider-picker" role="group" aria-label="Choose an account type">
            <span>Choose an account type:</span>
            <button type="button" className="btn" disabled={busyEmail !== null}
              onClick={() => runFor("__add__", async () => {
                await onAdd();
                restoreAddFocus.current = true;
                setAccountSetup(null);
              })}>
              Gmail
            </button>
            <button type="button" className="btn" disabled={busyEmail !== null}
              onClick={() => setAccountSetup("imap")}>
              IMAP
            </button>
          </div>
        )}
        {accountSetup === "imap" && <div>
          <ImapAccountSetup onCancel={() => {
            setAccountSetup(null);
            addButton.current?.focus();
          }} onConnected={async () => {
            await onImapConnected?.();
            setImapSaved(true);
            setAccountSetup(null);
            addButton.current?.focus();
          }} />
        </div>}
      </div>
      {imapSaved && <p role="status">Account saved. IMAP mail sync is not available yet.</p>}
      {accounts.length === 0 && authStatus && !authStatus.configured ? (
        <div className="notice accounts-config-notice">
          <CircleAlert size={ICON_SIZE.md} />
          <div>
            <strong>Google OAuth is not configured</strong>
            <p>
              Set <code>THREESTRANDS_GOOGLE_CLIENT_ID</code> and{" "}
              <code>THREESTRANDS_GOOGLE_CLIENT_SECRET</code> from a Google Desktop
              app credential, then restart ThreeStrands.
            </p>
          </div>
        </div>
      ) : null}
      {accounts.length === 0 ? (
        <div className="accounts-empty">
          <span className="accounts-empty-icon"><Mail size={ICON_SIZE.lg} /></span>
          <strong>No accounts connected</strong>
          <p>Add a Gmail account to start syncing mail on this device.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {accounts.map((account, index) => {
            const secondaryLine = [
              account.displayName ? account.email : null,
              account.lastSyncedAt ? `Last synced ${formatTimeOnly(account.lastSyncedAt)}` : "Not synced yet",
            ].filter(Boolean).join(" · ");
            return (
            <li className="account-card" key={account.email}>
              <div className="account-card-row">
                <span className="account-card-avatar" aria-hidden="true" style={{ background: account.color }}>
                  {(account.displayName ?? account.email).charAt(0).toUpperCase()}
                </span>
                <div className="account-card-identity">
                  <div className="account-card-heading">
                    <strong title={account.displayName ?? account.email}>{account.displayName ?? account.email}</strong>
                    <AccountStatusBadge status={account.status} />
                  </div>
                  <span className="account-card-email" title={secondaryLine}>{secondaryLine}</span>
                </div>
                {account.status === "needs_reauth" ? (
                  <button
                    type="button"
                    className="btn btn-sm btn-primary account-reconnect"
                    disabled={busyEmail !== null}
                    onClick={() => runFor(`reconnect:${account.email}`, () => onReconnect(account.email))}
                  >
                    {busyEmail === `reconnect:${account.email}` ? "Waiting for Google…" : "Reconnect"}
                  </button>
                ) : null}
                <span className="account-provider-badge">
                  {account.provider === "imap" ? "IMAP" : "Gmail"}
                </span>
              </div>
              <div className="account-card-controls">
                <AccountSenderNameInput
                  email={account.email}
                  name={account.displayName}
                  onCommit={(name) =>
                    onSetDisplayName(account.email, name).catch((reason: unknown) => {
                      setError(errorMessage(reason));
                      throw reason;
                    })
                  }
                />
                <span className="accounts-list-actions">
                  <span className="account-reorder">
                    <button className="btn-icon btn-icon-sm"
                      type="button"
                      aria-label={`Move ${account.email} up`}
                      disabled={index === 0 || busyEmail !== null}
                      onClick={() => move(index, -1)}
                    >
                      <ChevronUp size={ICON_SIZE.sm} />
                    </button>
                    <button className="btn-icon btn-icon-sm"
                      type="button"
                      aria-label={`Move ${account.email} down`}
                      disabled={index === accounts.length - 1 || busyEmail !== null}
                      onClick={() => move(index, 1)}
                    >
                      <ChevronDown size={ICON_SIZE.sm} />
                    </button>
                  </span>
                  <label className="account-color-swatch" title={`Color for ${account.email}`}>
                    <AccountColorInput
                      email={account.email}
                      color={account.color}
                      onCommit={(color) =>
                        onSetColor(account.email, color).catch((reason: unknown) => {
                          setError(errorMessage(reason));
                          throw reason;
                        })
                      }
                    />
                  </label>
                  <button
                    type="button"
                    className="btn btn-sm btn-danger"
                    disabled={busyEmail !== null}
                    aria-expanded={confirmEmail === account.email}
                    onClick={() => setConfirmEmail(account.email)}
                  >
                    Disconnect…
                  </button>
                </span>
              </div>
              {confirmEmail === account.email ? (
                <AccountDisconnectConfirm
                  kind="mail"
                  email={account.email}
                  disabled={busyEmail !== null}
                  onCancel={() => setConfirmEmail(null)}
                  onDisconnect={() => {
                    runFor(account.email, () => onRemove(account.email));
                    setConfirmEmail(null);
                  }}
                  onRemoveEverywhere={() => {
                    runFor(account.email, () => onRemoveEverywhere(account.email));
                    setConfirmEmail(null);
                  }}
                />
              ) : null}
            </li>
            );
          })}
        </ul>
      )}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

function AccountSenderNameInput({
  email,
  name,
  onCommit,
}: {
  email: string;
  name: string | null;
  onCommit(name: string | null): Promise<void>;
}) {
  const [value, setValue] = useState(name ?? "");
  const [saving, setSaving] = useState(false);
  const normalized = value.trim();
  const saved = name?.trim() ?? "";

  useEffect(() => setValue(name ?? ""), [name]);

  return (
    <form
      className="account-sender-name"
      onSubmit={(event) => {
        event.preventDefault();
        if (saving || normalized === saved) return;
        setSaving(true);
        void onCommit(normalized || null).finally(() => setSaving(false));
      }}
    >
      <input
        aria-label={`Sender name for ${email}`}
        value={value}
        maxLength={200}
        placeholder="Sender name"
        disabled={saving}
        onChange={(event) => setValue(event.target.value)}
      />
      <button className="btn btn-primary" type="submit" disabled={saving || normalized === saved}>
        {saving ? "Saving…" : "Save Name"}
      </button>
    </form>
  );
}

/**
 * A color swatch that saves on its own debounced schedule instead of on
 * every drag tick. `<input type="color">` fires `onChange` continuously
 * while the native picker is open, not just once on commit — driving that
 * straight into a save-and-disable cycle (the previous implementation) could
 * disable the input mid-drag and drop the rest of the gesture, so only the
 * first flicker of color ever got saved. Local `value` gives smooth
 * dragging; `color` (the saved value) is only adopted once no locally
 * committed save is still in flight, so a slow save can't snap the swatch
 * back to a stale color out from under the user.
 */
function AccountColorInput({
  email,
  color,
  onCommit,
}: {
  email: string;
  color: string;
  onCommit(color: string): Promise<void>;
}) {
  const [value, setValue] = useState(color);
  const pendingCount = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (pendingCount.current === 0) setValue(color);
  }, [color]);

  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);

  return (
    <input
      type="color"
      aria-label={`Color for ${email}`}
      value={value}
      onChange={(event) => {
        const next = event.target.value;
        setValue(next);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => {
          pendingCount.current++;
          onCommit(next)
            .catch(logBackgroundFailure("Account color save"))
            .finally(() => { pendingCount.current--; });
        }, 200);
      }}
    />
  );
}
