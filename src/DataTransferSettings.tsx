import { CircleCheck, Download, Upload } from "lucide-react";
import { useState } from "react";
import { exportSettings, importSettings, type SettingsImportResult } from "./userPreferences";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";

export function DataTransferSettings({
  onImported,
}: {
  onImported(result: SettingsImportResult): Promise<void>;
}) {
  const [exportPassword, setExportPassword] = useState("");
  const [exportConfirmation, setExportConfirmation] = useState("");
  const [importPassword, setImportPassword] = useState("");
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [message, setMessage] = useState<{ text: string; tone: "success" | "error" } | null>(null);
  const isDesktop = "__TAURI_INTERNALS__" in window;
  const passwordsMatch = exportPassword.length >= 8 && exportPassword === exportConfirmation;

  const showError = (error: unknown) => {
    setMessage({ text: errorMessage(error), tone: "error" });
  };

  return (
    <section className="settings-section wide-label-settings" aria-label="Data transfer">
      <h3>Export Settings and Accounts</h3>
      <p className="settings-hint">
        Creates a password-encrypted file containing your preferences, account
        list, Split Inboxes, and retention setting. Mail, OAuth credentials,
        API keys, and other keychain secrets are never exported.
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
        Importing replaces preferences and Split Inboxes from this installation.
        Existing connected accounts stay connected. Other imported accounts
        appear as “Connect on this device” and require Google authorization.
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
          void importSettings(importPassword)
            .then(async (result) => {
              if (!result) return;
              await onImported(result);
            })
            .catch(showError)
            .finally(() => setBusy(null));
        }}
      >
        <Upload size={ICON_SIZE.sm} aria-hidden="true" />
        {busy === "import" ? "Importing…" : "Choose Encrypted Settings File"}
      </button>
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
