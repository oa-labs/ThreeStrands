import { CircleCheck } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { mailClient } from "./data/client";
import type { DefaultAppRole, DefaultAppStatus } from "./domain";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { useSettingsOperation } from "./settingsOperations";

const ROLES: { role: DefaultAppRole; label: string; hint: string }[] = [
  { role: "mail", label: "Email Links", hint: "mailto: links in browsers and other apps start a new message here." },
  { role: "calendar", label: "Calendar Invitations", hint: ".ics files open here so you can answer them or add them to your calendar." },
];

/**
 * Makes ThreeStrands the macOS default for email links and calendar
 * invitations. macOS confirms each change with the user.
 */
export function DefaultAppsSettings() {
  const [status, setStatus] = useState<DefaultAppStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const { pending, error, errorKey, runFor } = useSettingsOperation();
  const refresh = useCallback(() => mailClient.defaultAppStatus()
    .then((next) => { setStatus(next); setLoadError(null); })
    .catch((reason: unknown) => setLoadError(errorMessage(reason))), []);

  // The default can also change in Finder or Mail, so check again on return.
  useEffect(() => {
    void refresh();
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [refresh]);

  return (
    <section className="settings-section" aria-label="Default Apps">
      <h3>Open in ThreeStrands</h3>
      {loadError ? <p className="form-error" role="alert">{loadError}</p> : null}
      {status && !status.supported ? (
        <p className="settings-hint">Default apps can be set from the installed ThreeStrands app on macOS.</p>
      ) : null}
      {status?.supported ? ROLES.map(({ role, label, hint }) => (
        <div key={role}>
          <div className="settings-field settings-field-inline">
            <span>{label}</span>
            <div className="settings-row">
              {status[role] ? (
                <span role="status" aria-label={`ThreeStrands is the default for ${label}`}>
                  <CircleCheck size={ICON_SIZE.sm} aria-hidden="true" /> Default
                </span>
              ) : (
                <button
                  type="button"
                  className="btn"
                  aria-label={`Make ThreeStrands the Default for ${label}`}
                  disabled={pending !== null}
                  onClick={() => runFor(role, async () => setStatus(await mailClient.makeDefaultApp(role)))}
                >
                  {pending === role ? "Waiting for macOS…" : "Make Default"}
                </button>
              )}
            </div>
          </div>
          <p className="settings-hint">{hint}</p>
          {error && errorKey === role ? <p className="form-error" role="alert">{error}</p> : null}
        </div>
      )) : null}
    </section>
  );
}
