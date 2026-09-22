import { useCallback, useEffect, useState, type ReactNode } from "react";
import { errorMessage } from "./errors";
import { FrontierConflictEditor } from "./FrontierConflictEditor";
import { holdRecoveryPhrase } from "./RecoveryPhraseDialog";
import {
  replicatedSyncAddFolder,
  replicatedSyncAddIpfsRpc,
  replicatedSyncApproveRequest,
  replicatedSyncBeginGenesis,
  replicatedSyncBetaEnabled,
  replicatedSyncConfirmEnrollment,
  replicatedSyncConflicts,
  replicatedSyncDeviceRoster,
  replicatedSyncEnabled,
  replicatedSyncEnrollmentStatus,
  replicatedSyncInspectSpace,
  replicatedSyncJoinWithRecoveryPhrase,
  replicatedSyncNow,
  replicatedSyncPendingRequests,
  replicatedSyncProbeIpfsRpc,
  replicatedSyncRejectRequest,
  replicatedSyncRemoveTransport,
  replicatedSyncResolveConflict,
  replicatedSyncRequestEnrollment,
  replicatedSyncRotateEpoch,
  replicatedSyncSetBetaEnabled,
  replicatedSyncStatus,
  type DeviceRosterEntry,
  type EnrollmentStatus,
  type FrontierConflict,
  type IncomingEnrollmentRequest,
  type IpfsRpcProbeReport,
  type ReplicatedSyncTransportStatus,
  type SyncSpacePresence,
} from "./replicatedSync";
import { useLiveStatus, useSettingsOperation } from "./settingsOperations";

const FILEBASE_RPC_URL = "https://rpc.filebase.io";

function formatStorageEstimate(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(1)} ${units[index]}`;
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

export type SetupStep = "location" | "choose" | "verify";

export const SETUP_STEPS: readonly { id: SetupStep; title: string; upcoming: string }[] = [
  { id: "location", title: "Choose where to sync", upcoming: "" },
  {
    id: "choose",
    title: "Join or create a sync space",
    upcoming: "Next, join the sync space your other devices use, or start a new one if this is your first device.",
  },
  {
    id: "verify",
    title: "Verify this device",
    upcoming: "If you join from another device, you’ll compare a short code on both screens before syncing starts.",
  },
];

/** Which setup step a not-yet-enrolled device is on. Enrollment progress
 * wins over locations: a device mid-verification stays there even if its
 * locations change underneath it. */
export function currentSetupStep(status: EnrollmentStatus | null, transportCount: number): SetupStep {
  if (status?.state === "awaitingGrant" || status?.state === "awaitingConfirmation") return "verify";
  return transportCount === 0 ? "location" : "choose";
}

export type SyncOverview = { tone: "ok" | "attention"; text: string };

/** One-line health summary for an enrolled device. A location needs
 * attention when its native health is anything but healthy or it has
 * undeliverable objects. */
export function syncOverview(transports: readonly ReplicatedSyncTransportStatus[], deviceCount: number): SyncOverview {
  const devices = plural(deviceCount, "device");
  if (transports.length === 0) {
    return { tone: "attention", text: `Not syncing · ${devices} · this device has no sync locations. Add one under Sync locations.` };
  }
  const troubled = transports.filter((transport) => transport.health !== "healthy" || transport.failed > 0).length;
  if (troubled > 0) {
    return { tone: "attention", text: `${devices} · ${plural(troubled, "sync location")} ${troubled === 1 ? "needs" : "need"} attention` };
  }
  const lastSuccess = transports
    .map((transport) => transport.lastSuccessAt)
    .filter((value): value is string => Boolean(value))
    .sort()
    .at(-1);
  return {
    tone: "ok",
    text: `Syncing · ${devices} · ${lastSuccess ? `last synced ${new Date(lastSuccess).toLocaleString()}` : "not synced yet"}`,
  };
}

export function ReplicatedSyncSettings() {
  const [available, setAvailable] = useState<boolean | null>(null);
  const [betaEnabled, setBetaEnabledState] = useState(false);
  const [transports, setTransports] = useState<ReplicatedSyncTransportStatus[]>([]);
  const [conflicts, setConflicts] = useState<FrontierConflict[]>([]);
  const [enrollmentStatus, setEnrollmentStatus] = useState<EnrollmentStatus | null>(null);
  const [pendingRequests, setPendingRequests] = useState<IncomingEnrollmentRequest[]>([]);
  const [deviceRoster, setDeviceRoster] = useState<DeviceRosterEntry[]>([]);
  const [spacePresence, setSpacePresence] = useState<SyncSpacePresence | "checking" | null>(null);
  const [presenceCheck, setPresenceCheck] = useState(0);

  const refresh = useCallback(async () => {
    const [enabled, beta] = await Promise.all([replicatedSyncEnabled(), replicatedSyncBetaEnabled()]);
    setAvailable(enabled);
    setBetaEnabledState(beta);
    if (enabled) {
      const [nextTransports, nextConflicts, nextStatus, nextPending, nextRoster] = await Promise.all([
        replicatedSyncStatus(),
        replicatedSyncConflicts(),
        replicatedSyncEnrollmentStatus(),
        replicatedSyncPendingRequests(),
        replicatedSyncDeviceRoster(),
      ]);
      setTransports(nextTransports);
      setConflicts(nextConflicts);
      setEnrollmentStatus(nextStatus);
      setPendingRequests(nextPending);
      setDeviceRoster(nextRoster);
    } else {
      setTransports([]);
      setConflicts([]);
      setEnrollmentStatus(null);
      setPendingRequests([]);
      setDeviceRoster([]);
    }
  }, []);

  const operation = useSettingsOperation(refresh);
  const { busy, error: message, setError: setMessage, run, act } = operation;
  const reportError = useCallback((reason: unknown) => setMessage(errorMessage(reason)), [setMessage]);
  useLiveStatus("replicated-sync-status", refresh, reportError);

  // Before this device joins or starts a space, look for one that another
  // device already published to the configured locations, so the setup
  // choice can steer toward joining it instead of forking a second space.
  const needsSetupChoice = available === true && enrollmentStatus?.state === "notStarted" && transports.length > 0;
  const transportKey = transports.map((transport) => transport.instanceId).join("\n");
  useEffect(() => {
    if (!needsSetupChoice) {
      setSpacePresence(null);
      return;
    }
    let cancelled = false;
    setSpacePresence("checking");
    replicatedSyncInspectSpace().then(
      (presence) => { if (!cancelled) setSpacePresence(presence); },
      () => { if (!cancelled) setSpacePresence("unknown"); },
    );
    return () => { cancelled = true; };
  }, [needsSetupChoice, transportKey, presenceCheck]);

  const beginGenesis = (allowExistingSpace: boolean) => run(async () => {
    try {
      holdRecoveryPhrase(await replicatedSyncBeginGenesis(allowExistingSpace));
    } finally {
      // A refusal means another device published a space since the last
      // check; re-inspect so the choice reflects it.
      setPresenceCheck((count) => count + 1);
    }
    await refresh();
  });

  const toggleBeta = (on: boolean) => act(() => replicatedSyncSetBetaEnabled(on));
  const betaToggle = (
    <label className="settings-field settings-field-inline">
      <input type="checkbox" checked={betaEnabled} disabled={busy} onChange={(event) => toggleBeta(event.target.checked)} />
      <span>Enable beta features</span>
    </label>
  );
  const status = message ? <p role="status" className="settings-hint">{message}</p> : null;

  if (available === null) {
    return (
      <section className="settings-section" aria-label="Replicated Sync">
        <p className="settings-hint">Loading replicated sync status…</p>
      </section>
    );
  }

  if (!available) {
    return (
      <section className="settings-section" aria-label="Replicated Sync">
        <h3>Replicated Sync (Beta)</h3>
        <p className="settings-hint">
          An in-development, end-to-end encrypted alternative to Three Strands Account sync, with no Three
          Strands-operated server: it replicates directly through folders or an IPFS endpoint you choose. Turn it on
          to set it up on this device.
        </p>
        {betaToggle}
        {status}
      </section>
    );
  }

  const locations = (
    <LocationManager transports={transports} operation={operation} refresh={refresh} />
  );
  const enrolled = enrollmentStatus?.state === "enrolled" ? enrollmentStatus : null;

  return (
    <section className="settings-section" aria-label="Replicated Sync">
      <h3>Replicated Sync (Beta)</h3>
      <details className="settings-disclosure">
        <summary>How replicated sync works</summary>
        <p className="settings-hint">
          Replicates tasks, snippets, Split Inboxes, and account metadata as end-to-end encrypted files through
          folders you choose. There is no Three Strands-operated sync server: ThreeStrands never sees the plaintext,
          but anyone with access to a selected folder can see the encrypted files themselves (their size and timing,
          not their contents). Deleting a folder here removes this device&apos;s copy of the synchronized data from
          that folder; it does not erase copies elsewhere (other devices, cloud provider version history, or other
          configured folders).
        </p>
      </details>

      {conflicts.length > 0 ? (
        <div>
          <h3>Resolve Conflicts</h3>
          {conflicts.map((conflict) => (
            <FrontierConflictEditor
              key={`${conflict.entityType}-${conflict.entityId}-${conflict.field}`}
              conflict={conflict}
              disabled={busy}
              onResolve={(chosen) => act(() => replicatedSyncResolveConflict(conflict, chosen))}
            />
          ))}
        </div>
      ) : null}

      {status}

      {enrolled ? (
        <EnrolledOverview
          deviceCount={enrolled.deviceCount}
          transports={transports}
          pendingRequests={pendingRequests}
          deviceRoster={deviceRoster}
          busy={busy}
          act={act}
          locations={locations}
          betaToggle={betaToggle}
        />
      ) : (
        <SetupSteps
          step={currentSetupStep(enrollmentStatus, transports.length)}
          enrollmentStatus={enrollmentStatus}
          transports={transports}
          spacePresence={spacePresence}
          busy={busy}
          run={run}
          act={act}
          refresh={refresh}
          beginGenesis={beginGenesis}
          locations={locations}
          betaToggle={betaToggle}
        />
      )}
    </section>
  );
}

type Operation = ReturnType<typeof useSettingsOperation>;
type Run = Operation["run"];

function SetupSteps({
  step,
  enrollmentStatus,
  transports,
  spacePresence,
  busy,
  run,
  act,
  refresh,
  beginGenesis,
  locations,
  betaToggle,
}: {
  step: SetupStep;
  enrollmentStatus: EnrollmentStatus | null;
  transports: ReplicatedSyncTransportStatus[];
  spacePresence: SyncSpacePresence | "checking" | null;
  busy: boolean;
  run: Run;
  act: Run;
  refresh(): Promise<void>;
  beginGenesis(allowExistingSpace: boolean): void;
  locations: ReactNode;
  betaToggle: ReactNode;
}) {
  const currentIndex = SETUP_STEPS.findIndex((candidate) => candidate.id === step);
  const bodies: Record<SetupStep, ReactNode> = {
    location: locations,
    choose: (
      <SetupChoice
        transportCount={transports.length}
        spacePresence={spacePresence}
        busy={busy}
        run={run}
        act={act}
        refresh={refresh}
        beginGenesis={beginGenesis}
      />
    ),
    verify: <VerifyStep enrollmentStatus={enrollmentStatus} busy={busy} act={act} />,
  };

  return (
    <>
      <ol className="sync-setup-steps" aria-label="Replicated sync setup">
        {SETUP_STEPS.map((candidate, index) => {
          const state = index < currentIndex ? "done" : index === currentIndex ? "current" : "upcoming";
          const headingId = `sync-setup-step-${candidate.id}`;
          return (
            <li
              key={candidate.id}
              className={`sync-setup-step sync-setup-step-${state}`}
              aria-current={state === "current" ? "step" : undefined}
              aria-labelledby={headingId}
            >
              <div className="sync-setup-step-header">
                <span className="sync-setup-step-marker" aria-hidden="true">{state === "done" ? "✓" : index + 1}</span>
                <h4 id={headingId}>{candidate.title}</h4>
                <span className="sync-setup-step-state">{state === "done" ? "Done" : state === "current" ? "Current step" : "Up next"}</span>
              </div>
              {state === "current" ? <div className="sync-setup-step-body">{bodies[candidate.id]}</div> : null}
              {state === "done" && candidate.id === "location" ? (
                <details className="settings-disclosure sync-setup-step-body">
                  <summary>{transports.length === 1 ? transports[0]!.location : plural(transports.length, "sync location")}</summary>
                  {locations}
                </details>
              ) : null}
              {state === "upcoming" && candidate.upcoming ? <p className="settings-hint sync-setup-step-body">{candidate.upcoming}</p> : null}
            </li>
          );
        })}
      </ol>
      <details className="settings-disclosure">
        <summary>Advanced</summary>
        {betaToggle}
      </details>
    </>
  );
}

function SetupChoice({
  transportCount,
  spacePresence,
  busy,
  run,
  act,
  refresh,
  beginGenesis,
}: {
  transportCount: number;
  spacePresence: SyncSpacePresence | "checking" | null;
  busy: boolean;
  run: Run;
  act: Run;
  refresh(): Promise<void>;
  beginGenesis(allowExistingSpace: boolean): void;
}) {
  const [recoveryPhraseInput, setRecoveryPhraseInput] = useState("");
  const [confirmingSeparateSpace, setConfirmingSeparateSpace] = useState(false);
  const noLocation = transportCount === 0;

  const joinWithPhrase = () => run(async () => {
    await replicatedSyncJoinWithRecoveryPhrase(recoveryPhraseInput.trim());
    setRecoveryPhraseInput("");
    await refresh();
  });

  return (
    <>
      <p className="settings-hint" role="status">
        {spacePresence === "checking"
          ? "Checking your sync locations for an existing sync space…"
          : spacePresence === "existing"
            ? "Another device already set up a sync space in this location. Join it to share data with your other devices."
            : spacePresence === "none"
              ? "No sync space found here yet. If this is your first device, create one. On your other devices, choose this same folder or endpoint, then join."
              : spacePresence === "unknown"
                ? "Couldn’t check every sync location for an existing space. If another device already syncs here, join it rather than creating a new one."
                : "Set this device up as the first device in a new encrypted sync space, or join a space that already exists on another device."}
      </p>
      {spacePresence === "existing" ? null : (
        <button
          type="button"
          className="primary-action"
          disabled={busy || noLocation || spacePresence === "checking"}
          onClick={() => beginGenesis(false)}
        >
          Create a new sync space
        </button>
      )}
      <button
        type="button"
        className={spacePresence === "existing" ? "primary-action" : "account-action-button"}
        disabled={busy || noLocation}
        onClick={() => act(replicatedSyncRequestEnrollment)}
      >
        Request to join from an existing device
      </button>
      <label className="settings-field">
        <span>Or join with a recovery phrase</span>
        <input
          type="text"
          placeholder="24 words separated by spaces"
          value={recoveryPhraseInput}
          disabled={busy}
          onChange={(event) => setRecoveryPhraseInput(event.target.value)}
        />
      </label>
      <button type="button" className="account-action-button" disabled={busy || !recoveryPhraseInput.trim() || noLocation} onClick={joinWithPhrase}>
        Join with recovery phrase
      </button>
      {spacePresence === "existing" ? (
        confirmingSeparateSpace ? (
          <div className="settings-inline-confirm" role="group" aria-label="Create a separate sync space confirmation">
            <p>
              <strong>Create a separate sync space?</strong><br />
              This device will not sync with the devices already using this location, and the new space gets its own
              recovery phrase.
            </p>
            <span className="settings-inline-confirm-actions">
              <button type="button" disabled={busy} onClick={() => setConfirmingSeparateSpace(false)}>Cancel</button>
              <button
                type="button"
                className="danger-action"
                disabled={busy}
                onClick={() => { setConfirmingSeparateSpace(false); beginGenesis(true); }}
              >
                Create separate space
              </button>
            </span>
          </div>
        ) : (
          <button type="button" className="account-action-button" disabled={busy} onClick={() => setConfirmingSeparateSpace(true)}>
            Create a separate sync space instead…
          </button>
        )
      ) : null}
    </>
  );
}

function VerifyStep({ enrollmentStatus, busy, act }: { enrollmentStatus: EnrollmentStatus | null; busy: boolean; act: Run }) {
  if (enrollmentStatus?.state === "awaitingGrant") {
    return (
      <>
        <p className="settings-hint">
          Open Replicated Sync on one of your existing devices and approve this device there. When it does, compare
          fingerprints on both screens before confirming. This device&apos;s fingerprint:{" "}
          <strong style={{ fontFamily: "monospace" }}>{enrollmentStatus.fingerprint}</strong>
        </p>
        <button type="button" className="account-action-button" disabled={busy} onClick={() => act(replicatedSyncNow)}>
          Check for approval
        </button>
      </>
    );
  }
  if (enrollmentStatus?.state === "awaitingConfirmation") {
    const requestId = enrollmentStatus.requestId;
    return (
      <div className="settings-field">
        <p className="settings-hint">
          An approval arrived. Compare these fingerprints with what the approving device shows — they must match
          exactly before you confirm.
        </p>
        <p style={{ fontFamily: "monospace" }}>This device: {enrollmentStatus.fingerprint}</p>
        <p style={{ fontFamily: "monospace" }}>Approver: {enrollmentStatus.approverFingerprint}</p>
        <button type="button" className="primary-action" disabled={busy} onClick={() => act(() => replicatedSyncConfirmEnrollment(requestId))}>
          Confirm — fingerprints match
        </button>
      </div>
    );
  }
  return null;
}

function EnrolledOverview({
  deviceCount,
  transports,
  pendingRequests,
  deviceRoster,
  busy,
  act,
  locations,
  betaToggle,
}: {
  deviceCount: number;
  transports: ReplicatedSyncTransportStatus[];
  pendingRequests: IncomingEnrollmentRequest[];
  deviceRoster: DeviceRosterEntry[];
  busy: boolean;
  act: Run;
  locations: ReactNode;
  betaToggle: ReactNode;
}) {
  const [revokingDevice, setRevokingDevice] = useState<string | null>(null);
  const overview = syncOverview(transports, deviceCount);

  return (
    <>
      <div className={`sync-overview sync-overview-${overview.tone}`} role="status" aria-label="Sync status">
        <span>{overview.text}</span>
        <button type="button" className="account-action-button" disabled={busy || transports.length === 0} onClick={() => act(replicatedSyncNow)}>
          Sync now
        </button>
      </div>

      {pendingRequests.length > 0 ? (
        <>
          <h4>Devices waiting to join</h4>
          <ul className="accounts-list" aria-label="Devices waiting to join">
            {pendingRequests.map((request) => (
              <li className="account-card" key={request.requestId}>
                <div className="account-card-row">
                  <div className="account-card-identity">
                    <strong style={{ fontFamily: "monospace" }}>{request.fingerprint}</strong>
                    <span className="account-card-email">Requested {new Date(request.createdAt).toLocaleString()}</span>
                  </div>
                  <button type="button" className="primary-action" disabled={busy} onClick={() => act(() => replicatedSyncApproveRequest(request.requestId))}>
                    Approve
                  </button>
                  <button type="button" className="account-action-button danger-action" disabled={busy} onClick={() => act(() => replicatedSyncRejectRequest(request.requestId))}>
                    Reject
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </>
      ) : null}

      <details className="settings-disclosure" open>
        <summary>Devices ({deviceRoster.length})</summary>
        <ul className="accounts-list">
          {deviceRoster.map((device) => (
            <li className="account-card" key={device.deviceId}>
              <div className="account-card-row">
                <div className="account-card-identity">
                  <strong style={{ fontFamily: "monospace" }}>{device.deviceId}</strong>
                  <span className="account-card-email">
                    {device.status}
                    {device.isSelf ? " · this device" : ""}
                  </span>
                </div>
                {!device.isSelf && device.status === "active" ? (
                  <button type="button" className="account-action-button danger-action" disabled={busy} aria-expanded={revokingDevice === device.deviceId} onClick={() => setRevokingDevice(device.deviceId)}>
                    Revoke…
                  </button>
                ) : null}
              </div>
              {revokingDevice === device.deviceId ? (
                <div className="settings-inline-confirm" role="group" aria-label="Revoke device confirmation">
                  <p><strong>Revoke this device?</strong><br />It keeps existing data, but future writes from it will no longer be trusted.</p>
                  <span className="settings-inline-confirm-actions">
                    <button type="button" disabled={busy} onClick={() => setRevokingDevice(null)}>Cancel</button>
                    <button type="button" className="danger-action" disabled={busy} onClick={() => { setRevokingDevice(null); act(() => replicatedSyncRotateEpoch(device.deviceId)); }}>Revoke device</button>
                  </span>
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      </details>

      <details className="settings-disclosure" open={overview.tone === "attention"}>
        <summary>Sync locations ({transports.length})</summary>
        {locations}
      </details>

      <details className="settings-disclosure">
        <summary>Advanced</summary>
        {betaToggle}
      </details>
    </>
  );
}

/** The configured locations plus the controls to add another. Rendered as
 * step 1 during setup and under "Sync locations" once enrolled. */
function LocationManager({
  transports,
  operation,
  refresh,
}: {
  transports: ReplicatedSyncTransportStatus[];
  operation: Operation;
  refresh(): Promise<void>;
}) {
  const { busy, setError: setMessage, run, act } = operation;
  const [ipfsBaseUrl, setIpfsBaseUrl] = useState("");
  const [ipfsToken, setIpfsToken] = useState("");
  const [ipfsProbe, setIpfsProbe] = useState<IpfsRpcProbeReport | null>(null);
  const [disconnectingTransport, setDisconnectingTransport] = useState<string | null>(null);

  const addFolder = () => run(async () => {
    const status = await replicatedSyncAddFolder();
    if (!status) setMessage("No folder selected.");
    await refresh();
  });

  const probeIpfsRpc = () => {
    setIpfsProbe(null);
    run(async () => {
      const report = await replicatedSyncProbeIpfsRpc(ipfsBaseUrl, ipfsToken.trim() ? ipfsToken : null);
      setIpfsProbe(report);
      if (!report.versionOk) setMessage("Could not reach an IPFS RPC endpoint at that URL.");
    });
  };

  const addIpfsRpc = () => run(async () => {
    const status = await replicatedSyncAddIpfsRpc(ipfsBaseUrl, ipfsToken.trim() ? ipfsToken : null);
    if (!status) setMessage("Could not add that endpoint.");
    setIpfsBaseUrl("");
    setIpfsToken("");
    setIpfsProbe(null);
    await refresh();
  });

  return (
    <>
      <p className="settings-hint">
        Every device you sync must use the same location. Choose a folder your devices already share, such as one in
        a cloud drive or on a network share, and pick that same folder on each device.
      </p>
      {transports.length > 0 ? (
        <ul className="accounts-list" aria-label="Sync locations">
          {transports.map((transport) => (
            <li className="account-card" key={transport.instanceId}>
              <div className="account-card-row">
                <div className="account-card-identity">
                  <strong>{transport.location}</strong>
                  <span className="account-card-email">
                    {transport.kind === "ipfs_rpc" ? "IPFS RPC" : "Folder"}
                    {!transport.headDiscovery ? " · storage-only" : ""} · {transport.health} · {transport.pending} pending
                    {transport.failed ? `, ${transport.failed} failed` : ""}
                    {transport.storageBytes != null ? ` · ${formatStorageEstimate(transport.storageBytes)}` : ""}
                  </span>
                  {transport.lastSuccessAt ? (
                    <span className="account-card-email">Last synced {new Date(transport.lastSuccessAt).toLocaleString()}</span>
                  ) : null}
                  {transport.lastError ? <span className="account-card-email">{transport.lastError}</span> : null}
                </div>
                <button
                  type="button"
                  className="account-action-button danger-action"
                  disabled={busy}
                  aria-expanded={disconnectingTransport === transport.instanceId}
                  onClick={() => setDisconnectingTransport(transport.instanceId)}
                >
                  Disconnect…
                </button>
              </div>
              {disconnectingTransport === transport.instanceId ? (
                <div className="settings-inline-confirm" role="group" aria-label="Disconnect sync transport confirmation">
                  <p>
                    <strong>Stop syncing to this {transport.kind === "ipfs_rpc" ? "endpoint" : "folder"}?</strong><br />
                    {transport.kind === "ipfs_rpc"
                      ? "Pinned objects remain with the provider until you remove them there."
                      : "You can keep the encrypted files for another device or delete this device’s copy."}
                  </p>
                  <span className="settings-inline-confirm-actions">
                    <button type="button" disabled={busy} onClick={() => setDisconnectingTransport(null)}>Cancel</button>
                    <button type="button" disabled={busy} onClick={() => { setDisconnectingTransport(null); act(() => replicatedSyncRemoveTransport(transport.instanceId, false)); }}>Disconnect and keep data</button>
                    {transport.kind !== "ipfs_rpc" ? (
                      <button type="button" className="danger-action" disabled={busy} onClick={() => { setDisconnectingTransport(null); act(() => replicatedSyncRemoveTransport(transport.instanceId, true)); }}>Delete files and disconnect</button>
                    ) : null}
                  </span>
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}

      <button type="button" className={transports.length === 0 ? "primary-action" : "account-action-button"} disabled={busy} onClick={addFolder}>
        {transports.length === 0 ? "Add a sync folder" : "Add another sync folder"}
      </button>

      <details className="settings-disclosure">
        <summary>Use an IPFS RPC endpoint instead</summary>
        <p className="settings-hint">
          Advanced: point at a Kubo-compatible RPC endpoint (for example a Filebase bucket, or a local Kubo daemon) to
          replicate through it instead of, or alongside, a folder. The access token, if any, is stored only in this
          device&apos;s OS keychain, never in a settings export.
        </p>
        <label className="settings-field">
          <span>RPC base URL</span>
          <input
            type="text"
            placeholder="https://rpc.filebase.io"
            value={ipfsBaseUrl}
            disabled={busy}
            onChange={(event) => {
              setIpfsBaseUrl(event.target.value);
              setIpfsProbe(null);
            }}
          />
        </label>
        <label className="settings-field">
          <span>Access token (optional)</span>
          <input
            type="password"
            value={ipfsToken}
            disabled={busy}
            onChange={(event) => {
              setIpfsToken(event.target.value);
              setIpfsProbe(null);
            }}
          />
        </label>
        <button
          type="button"
          className="account-action-button"
          disabled={busy}
          onClick={() => {
            setIpfsBaseUrl(FILEBASE_RPC_URL);
            setIpfsProbe(null);
          }}
        >
          Use Filebase preset
        </button>
        <button type="button" className="account-action-button" disabled={busy || !ipfsBaseUrl} onClick={probeIpfsRpc}>
          Test connection
        </button>
        {ipfsProbe ? (
          <p className="settings-hint">
            {ipfsProbe.versionOk
              ? `Reachable · ${ipfsProbe.mfsAvailable ? "supports discovery (MFS)" : "storage-only, no MFS discovery"}`
              : "Not reachable at that URL."}
          </p>
        ) : null}
        <button type="button" className="primary-action" disabled={busy || !ipfsBaseUrl} onClick={addIpfsRpc}>
          Add IPFS RPC endpoint
        </button>
      </details>
    </>
  );
}
