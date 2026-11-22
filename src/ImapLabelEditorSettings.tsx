import { useEffect, useState } from "react";
import { Modal } from "./AppChrome";
import { errorMessage } from "./errors";
import {
  handoffImapLabelEditor,
  imapLabelEditor,
  replicatedSyncDeviceRoster,
  type DeviceRosterEntry,
  type ImapLabelEditorStatus,
} from "./replicatedSync";

function deviceName(device: DeviceRosterEntry): string {
  return device.label || (device.isSelf ? "This device" : `Device ${device.deviceId.slice(0, 8)}`);
}

export function ImapLabelEditorSettings({ accountId, onClose }: {
  accountId: string;
  onClose(): void;
}) {
  const [status, setStatus] = useState<ImapLabelEditorStatus | null>(null);
  const [roster, setRoster] = useState<DeviceRosterEntry[]>([]);
  const [recipient, setRecipient] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    void Promise.all([imapLabelEditor(accountId), replicatedSyncDeviceRoster()]).then(
      ([nextStatus, nextRoster]) => {
        if (current) {
          setStatus(nextStatus);
          setRoster(nextRoster);
        }
      },
      (reason: unknown) => { if (current) setError(errorMessage(reason)); },
    );
    return () => { current = false; };
  }, [accountId]);

  const changeDevice = async () => {
    setBusy(true);
    setError(null);
    try {
      await handoffImapLabelEditor(accountId, recipient);
      onClose();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const editor = roster.find((device) => device.deviceId === status?.designated);
  const peers = roster.filter((device) =>
    !device.isSelf && device.status === "active" && status?.eligibleDevices.includes(device.deviceId),
  );
  return (
    <Modal title="IMAP Editing Device" onClose={onClose}>
      {status ? (
        <p className="settings-hint">
          {editor
            ? `${deviceName(editor)} is designated to rename and delete IMAP labels and folders for ${accountId}.`
            : "The editing device is unavailable. Sync your enrolled devices to check the assignment."}
        </p>
      ) : error === null ? <p role="status">Loading…</p> : null}
      {status?.hasPeers && !status.ready ? (
        <p className="settings-hint">Update and sync every enrolled device before editing.</p>
      ) : null}
      {status?.isEditor && status.ready && peers.length ? (
        <form onSubmit={(event) => { event.preventDefault(); void changeDevice(); }}>
          <label>
            New editing device{" "}
            <select required value={recipient} disabled={busy} onChange={(event) => setRecipient(event.target.value)}>
              <option value="">Choose a device</option>
              {peers.map((device) => <option key={device.deviceId} value={device.deviceId}>{deviceName(device)}</option>)}
            </select>
          </label>{" "}
          <button type="submit" className="btn btn-sm" disabled={!recipient || busy}>Change Editing Device</button>
        </form>
      ) : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </Modal>
  );
}
