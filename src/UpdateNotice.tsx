import { useCallback, useEffect, useRef, useState } from "react";
import { Download, RefreshCw, X } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { checkForAppUpdate, installAppUpdate, type AvailableUpdate } from "./appUpdate";
import { logBackgroundFailure } from "./errors";

/** How often a running app looks for a newer release after the launch check. */
export const UPDATE_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

const reportCheckFailure = logBackgroundFailure("Checking for app updates");
const reportOpenFailure = logBackgroundFailure("Opening the release page");

type InstallState = "idle" | "installing" | "failed";

/**
 * An app-wide prompt when a newer ThreeStrands is published. Nothing installs
 * until the user chooses to; installs that cannot replace themselves (deb and
 * rpm packages, or a Mac app still on its disk image) get a download link
 * instead. Dismissing hides that version until the app next launches.
 */
export function UpdateNotice() {
  const [update, setUpdate] = useState<AvailableUpdate | null>(null);
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);
  const [installState, setInstallState] = useState<InstallState>("idle");
  const installing = useRef(false);

  const check = useCallback(async () => {
    // A check finding nothing must not clear a prompt mid-install.
    if (installing.current) return;
    const found = await checkForAppUpdate();
    if (found && typeof found.version === "string") setUpdate(found);
  }, []);

  useEffect(() => {
    check().catch(reportCheckFailure);
    const interval = window.setInterval(() => check().catch(reportCheckFailure), UPDATE_CHECK_INTERVAL_MS);
    return () => window.clearInterval(interval);
  }, [check]);

  if (!update || update.version === dismissedVersion) return null;

  const openRelease = () => openUrl(update.releaseUrl).catch(reportOpenFailure);
  const install = async () => {
    installing.current = true;
    setInstallState("installing");
    try {
      // Resolves only on failure; success relaunches the app.
      await installAppUpdate();
    } catch (error) {
      console.warn("Installing the app update failed:", error);
      setInstallState("failed");
    } finally {
      installing.current = false;
    }
  };

  if (installState === "installing") {
    return (
      <div className="toast update-toast" role="status" aria-live="polite">
        <RefreshCw className="spin update-toast-icon" size={16} aria-hidden="true" />
        <span className="update-toast-message">Installing ThreeStrands {update.version}. The app will restart when it's done.</span>
      </div>
    );
  }

  const dismiss = (
    <button aria-label="Dismiss update" onClick={() => { setDismissedVersion(update.version); setInstallState("idle"); }}>
      <X size={14} />
    </button>
  );

  if (installState === "failed") {
    return (
      <div className="toast update-toast" role="alert">
        <Download className="update-toast-icon" size={18} aria-hidden="true" />
        <span className="update-toast-message">ThreeStrands {update.version} couldn't be installed.</span>
        <button onClick={install}>Try again</button>
        <button onClick={openRelease}>Download</button>
        {dismiss}
      </div>
    );
  }

  return (
    <div className="toast update-toast" role="status" aria-live="polite">
      <Download className="update-toast-icon" size={18} aria-hidden="true" />
      <span className="update-toast-message">
        ThreeStrands {update.version} is available.
        {update.installMode === "moveToApplications"
          ? " Move ThreeStrands to your Applications folder to install updates automatically."
          : null}
      </span>
      {update.installMode === "inPlace" ? (
        <>
          <button onClick={openRelease}>What's new</button>
          <button onClick={install}>Install and restart</button>
        </>
      ) : (
        <button onClick={openRelease}>Download</button>
      )}
      {dismiss}
    </div>
  );
}
