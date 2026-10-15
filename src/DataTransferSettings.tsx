import { CircleCheck, Download, Upload } from "lucide-react";
import { useState } from "react";
import {
  applySettingsImport,
  cancelSettingsImport,
  exportSettings,
  previewSettingsImport,
  type SettingsImportPreview,
  type SettingsImportResult,
} from "./userPreferences";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";

/**
 * What the encrypted settings file carries, keyed by the export payload's
 * field in `src-tauri/src/transfer.rs`. A test keeps this list in step with
 * that payload, so the hint never understates what leaves the computer.
 */
export const TRANSFERRED_DATA = [
  { field: "preferences", label: "preferences", replacedOnImport: true },
  { field: "accounts", label: "account list", replacedOnImport: false },
  { field: "split_inboxes", label: "Split Inboxes", replacedOnImport: true },
  { field: "signatures", label: "signatures and their defaults", replacedOnImport: true },
  { field: "snippets", label: "snippets", replacedOnImport: true },
  { field: "contacts", label: "saved contacts", replacedOnImport: true },
  { field: "contact_groups", label: "contact groups", replacedOnImport: true },
  { field: "sender_rules", label: "muted senders", replacedOnImport: true },
  { field: "retention_days", label: "retention setting", replacedOnImport: true },
] as const;

function listPhrase(items: readonly string[]): string {
  return items.length < 2 ? items.join("") : `${items.slice(0, -1).join(", ")}, and ${items.at(-1)}`;
}

export function DataTransferSettings({
  onImported,
}: {
  onImported(result: SettingsImportResult): Promise<void>;
}) {
  const [exportPassword, setExportPassword] = useState("");
  const [exportConfirmation, setExportConfirmation] = useState("");
  const [importPassword, setImportPassword] = useState("");
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  // A decrypted file waiting for the user to pick which accounts to set up here.
  const [preview, setPreview] = useState<SettingsImportPreview | null>(null);
  const [notHere, setNotHere] = useState<ReadonlySet<string>>(new Set());
  const [message, setMessage] = useState<{ text: string; tone: "success" | "error" } | null>(null);
  const isDesktop = "__TAURI_INTERNALS__" in window;
  const passwordsMatch = exportPassword.length >= 8 && exportPassword === exportConfirmation;

  const showError = (error: unknown) => {
    setMessage({ text: errorMessage(error), tone: "error" });
  };

  // The native side consumes the staged file either way, so a failure means
  // choosing the file again rather than retrying the same choices.
  const applyImport = async (skipped: string[]) => {
    setPreview(null);
    const result = await applySettingsImport(skipped);
    setImportPassword("");
    await onImported(result);
  };

  return (
    <section className="settings-section wide-label-settings" aria-label="Data transfer">
      <h3>Export Settings and Accounts</h3>
      <p className="settings-hint">
        Creates a password-encrypted file containing your {listPhrase(TRANSFERRED_DATA.map((item) => item.label))}.
        Mail, tasks, OAuth credentials, passwords, API keys, and other keychain
        secrets are never exported.
      </p>
      <label className="settings-field settings-field-row">
        <span>Export Password</span>
        <input
          type="password"
          autoComplete="new-password"
          value={exportPassword}
          onChange={(event) => setExportPassword(event.target.value)}
          disabled={!isDesktop || busy !== null}
        />
      </label>
      <label className="settings-field settings-field-row">
        <span>Confirm Password</span>
        <input
          type="password"
          autoComplete="new-password"
          value={exportConfirmation}
          onChange={(event) => setExportConfirmation(event.target.value)}
          disabled={!isDesktop || busy !== null}
        />
      </label>
      {exportPassword.length > 0 && exportPassword.length < 8 ? (
        <span className="settings-hint settings-field-detail">Use at least 8 characters.</span>
      ) : exportConfirmation.length > 0 && exportPassword !== exportConfirmation ? (
        <span className="settings-hint settings-field-detail">Passwords don’t match.</span>
      ) : null}
      <button
        type="button"
        className="btn settings-field-offset"
        disabled={!isDesktop || !passwordsMatch || busy !== null}
        onClick={() => {
          setBusy("export");
          setMessage(null);
          void exportSettings(exportPassword)
            .then((path) => {
              if (path) {
                setMessage({ text: `Settings exported to ${path}`, tone: "success" });
                setExportPassword("");
                setExportConfirmation("");
              }
            })
            .catch(showError)
            .finally(() => setBusy(null));
        }}
      >
        <Download size={ICON_SIZE.sm} aria-hidden="true" />
        {busy === "export" ? "Exporting…" : "Export Encrypted Settings"}
      </button>

      <h3>Import Settings and Accounts</h3>
      <p className="settings-hint">
        Importing replaces the {listPhrase(TRANSFERRED_DATA.filter((item) => item.replacedOnImport).map((item) => item.label))} on
        this installation. Existing connected accounts stay connected. You
        choose which other imported accounts to set up on this device; they
        appear as “Connect on this device” until you sign in to them again.
      </p>
      <label className="settings-field settings-field-row">
        <span>Backup File Password</span>
        <input
          type="password"
          autoComplete="current-password"
          aria-label="Backup File Password"
          value={importPassword}
          onChange={(event) => setImportPassword(event.target.value)}
          disabled={!isDesktop || busy !== null}
        />
      </label>
      <button
        type="button"
        className="btn settings-field-offset"
        disabled={!isDesktop || importPassword.length < 8 || busy !== null}
        onClick={() => {
          setBusy("import");
          setMessage(null);
          void previewSettingsImport(importPassword)
            .then(async (next) => {
              if (!next) return;
              // Nothing to choose when every account is already set up here.
              if (next.accounts.every((account) => account.setUpHere)) {
                await applyImport([]);
                return;
              }
              setNotHere(new Set());
              setPreview(next);
            })
            .catch(showError)
            .finally(() => setBusy(null));
        }}
      >
        <Upload size={ICON_SIZE.sm} aria-hidden="true" />
        {busy === "import" ? "Importing…" : "Choose Encrypted Settings File"}
      </button>
      {preview ? (
        <fieldset className="import-account-choices" disabled={busy !== null}>
          <legend>Accounts to Set Up on This Device</legend>
          <p className="settings-hint">
            Unchecked accounts are listed under “On Your Other Devices” in Mail Accounts, where you can set them up later.
          </p>
          {preview.accounts.map((account) => {
            const name = account.nickname?.trim() || account.displayName || account.email;
            return (
              <label key={account.email} className="settings-checkbox">
                <input
                  type="checkbox"
                  checked={account.setUpHere || !notHere.has(account.email)}
                  disabled={account.setUpHere}
                  onChange={(event) => {
                    const skip = !event.target.checked;
                    setNotHere((current) => {
                      const next = new Set(current);
                      if (skip) next.add(account.email);
                      else next.delete(account.email);
                      return next;
                    });
                  }}
                />
                <span>
                  {name}{name !== account.email ? ` (${account.email})` : ""} · {account.provider === "imap" ? "IMAP" : "Gmail"}
                  {account.setUpHere ? " · Already set up here" : ""}
                </span>
              </label>
            );
          })}
          <div className="import-account-actions">
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                setBusy("import");
                setMessage(null);
                void applyImport([...notHere]).catch(showError).finally(() => setBusy(null));
              }}
            >
              Import
            </button>
            <button
              type="button"
              className="btn"
              onClick={() => {
                setPreview(null);
                void cancelSettingsImport().catch(showError);
              }}
            >
              Cancel
            </button>
          </div>
        </fieldset>
      ) : null}
      {!isDesktop ? (
        <p className="settings-hint" role="status">
          Settings transfer is available in the ThreeStrands desktop app.
        </p>
      ) : null}
      {message?.tone === "error" ? <p className="form-error" role="alert">{message.text}</p> : null}
      {message?.tone === "success" ? (
        <p className="settings-connection-status configured" role="status">
          <CircleCheck size={ICON_SIZE.xs} aria-hidden="true" /> {message.text}
        </p>
      ) : null}
    </section>
  );
}
