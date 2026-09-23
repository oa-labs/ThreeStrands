import { useCallback, useEffect, useId, useState, type ReactNode } from "react";
import { errorMessage } from "./errors";
import { FrontierConflictEditor } from "./FrontierConflictEditor";
import { holdRecoveryPhrase } from "./RecoveryPhraseDialog";
import {
  MAX_DEVICE_LABEL_CHARS,
  replicatedSyncAddFolder,
  replicatedSyncAddIpfsRpc,
  replicatedSyncApproveRequest,
  replicatedSyncBeginGenesis,
  replicatedSyncBetaEnabled,
  replicatedSyncCheckRecoveryPhrase,
  replicatedSyncConfirmEnrollment,
  replicatedSyncConflicts,
  replicatedSyncDeviceRoster,
  replicatedSyncEnabled,
  replicatedSyncEnrollmentStatus,
  replicatedSyncInspectSpace,
  replicatedSyncJoinWithRecoveryPhrase,
  replicatedSyncLeave,
  replicatedSyncNow,
  replicatedSyncPendingRequests,
  replicatedSyncProbeIpfsRpc,
  replicatedSyncRejectRequest,
  replicatedSyncRemoveTransport,
  replicatedSyncResolveConflict,
  replicatedSyncRequestEnrollment,
  replicatedSyncRotateEpoch,
  replicatedSyncSetBetaEnabled,
  replicatedSyncSetDeviceLabel,
  replicatedSyncStatus,
  type DeviceRosterEntry,
  type EnrollmentStatus,
  type FrontierConflict,
  type IncomingEnrollmentRequest,
  type IpfsRpcProbeReport,
  type RecoveryPhraseCheck,
  type ReplicatedSyncTransportStatus,
  type SyncSpacePresence,
} from "./replicatedSync";
import { ANY_OPERATION, useLiveStatus, useSettingsOperation } from "./settingsOperations";

const FILEBASE_RPC_URL = "https://rpc.filebase.io";
const RECOVERY_PHRASE_WORDS = 24;

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

/** A collapsible settings group. Content sits in its own flex body because
 * WebKit does not lay out `<details>` children as flex items. */
function Disclosure({
  summary,
  open,
  className,
  children,
}: {
  summary: ReactNode;
  open?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <details className={className ? `settings-disclosure ${className}` : "settings-disclosure"} open={open}>
      <summary>{summary}</summary>
      <div className="settings-disclosure-body">{children}</div>
    </details>
  );
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

function listPositions(positions: number[]): string {
  const numbers = positions.map((position) => String(position + 1));
  if (numbers.length <= 1) return numbers.join("");
  return `${numbers.slice(0, -1).join(", ")} and ${numbers.at(-1)}`;
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

export type Tone = "ok" | "attention";
export type TransportHealthSummary = { tone: Tone; label: string; detail: string | null };

/** Plain-language state for one location. The native side reports
 * `healthy`, `degraded: <reason>`, or `unavailable: <reason>`. */
export function describeTransportHealth(transport: ReplicatedSyncTransportStatus): TransportHealthSummary {
  const separator = transport.health.indexOf(": ");
  const state = separator === -1 ? transport.health : transport.health.slice(0, separator);
  const reason = separator === -1 ? null : transport.health.slice(separator + 2);
  if (state === "unavailable") {
    return reason === "not configured"
      ? { tone: "attention", label: "Not set up correctly. Disconnect it and add it again.", detail: null }
      : { tone: "attention", label: "Can’t reach this location", detail: reason };
  }
  if (state === "degraded") return { tone: "attention", label: "Having trouble, retrying automatically", detail: reason };
  if (state !== "healthy") return { tone: "attention", label: "Status unknown", detail: transport.health };
  if (transport.failed > 0) {
    return { tone: "attention", label: `${plural(transport.failed, "change")} couldn’t be uploaded`, detail: transport.lastError ?? null };
  }
  if (transport.pending > 0) return { tone: "ok", label: `Uploading ${plural(transport.pending, "change")}`, detail: null };
  return { tone: "ok", label: "Up to date", detail: null };
}

export type SyncOverview = { tone: Tone; text: string };

/** One-line health summary for an enrolled device. */
export function syncOverview(transports: readonly ReplicatedSyncTransportStatus[], deviceCount: number): SyncOverview {
  const devices = plural(deviceCount, "device");
  if (transports.length === 0) {
    return { tone: "attention", text: `Not syncing · ${devices} · this device has no sync locations. Add one under Sync locations.` };
  }
  const troubled = transports.filter((transport) => describeTransportHealth(transport).tone === "attention").length;
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

export function deviceDisplayName(device: DeviceRosterEntry): string {
  return device.label || (device.isSelf ? "This device" : "Unnamed device");
}

/** What to tell someone typing a recovery phrase. The word still being
 * typed is only flagged once it is followed by a space or the phrase is
 * complete, so a half-typed word never shows as a mistake. */
export function recoveryPhraseFeedback(input: string, check: RecoveryPhraseCheck | null): { tone: Tone; text: string } | null {
  if (!check || check.wordCount === 0) return null;
  const finishedWords = /\s$/.test(input) || check.wordCount >= RECOVERY_PHRASE_WORDS ? check.wordCount : check.wordCount - 1;
  const flagged = check.unknownWordPositions.filter((position) => position < finishedWords);
  if (flagged.length === 1) return { tone: "attention", text: `Word ${listPositions(flagged)} isn’t a recovery phrase word. Check its spelling.` };
  if (flagged.length > 1) return { tone: "attention", text: `Words ${listPositions(flagged)} aren’t recovery phrase words. Check their spelling.` };
  if (check.wordCount > RECOVERY_PHRASE_WORDS) return { tone: "attention", text: `That’s ${check.wordCount} words. A recovery phrase has ${RECOVERY_PHRASE_WORDS}.` };
  if (check.valid) return { tone: "ok", text: "Recovery phrase looks right." };
  if (check.wordCount === RECOVERY_PHRASE_WORDS && check.unknownWordPositions.length === 0) {
    return { tone: "attention", text: "All 24 words are recognized, but they don’t form a valid phrase. Check their order and spelling." };
  }
  return { tone: "ok", text: `${check.wordCount} of ${RECOVERY_PHRASE_WORDS} words` };
}

type Operation = ReturnType<typeof useSettingsOperation>;

/** The failure from one keyed operation, shown next to its control. */
function InlineStatus({ operation, for: key }: { operation: Operation; for: string }) {
  return operation.error && operation.errorKey === key
    ? <p role="status" className="settings-hint settings-inline-status">{operation.error}</p>
    : null;
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
  const { busy, setError, runFor, actFor } = operation;
  const reportError = useCallback((reason: unknown) => setError(errorMessage(reason)), [setError]);
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

  const beginGenesis = (allowExistingSpace: boolean) => runFor("genesis", async () => {
    try {
      holdRecoveryPhrase(await replicatedSyncBeginGenesis(allowExistingSpace));
    } finally {
      // A refusal means another device published a space since the last
      // check; re-inspect so the choice reflects it.
      setPresenceCheck((count) => count + 1);
    }
    await refresh();
  });

  const betaToggle = (
    <>
      <label className="settings-checkbox">
        <input type="checkbox" checked={betaEnabled} disabled={busy} onChange={(event) => actFor("beta", () => replicatedSyncSetBetaEnabled(event.target.checked))} />
        <span>Enable beta features</span>
      </label>
      <InlineStatus operation={operation} for="beta" />
    </>
  );
  const sectionStatus = operation.error && operation.errorKey === ANY_OPERATION
    ? <p role="status" className="settings-hint">{operation.error}</p>
    : null;

  if (available === null) {
    return (
      <section className="settings-section" aria-label="Replicated Sync">
        <p className="settings-hint">Loading replicated sync status…</p>
        {sectionStatus}
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
        {sectionStatus}
      </section>
    );
  }

  const locations = <LocationManager transports={transports} operation={operation} refresh={refresh} />;
  const enrolled = enrollmentStatus?.state === "enrolled" ? enrollmentStatus : null;

  return (
    <section className="settings-section" aria-label="Replicated Sync">
      <h3>Replicated Sync (Beta)</h3>
      <Disclosure summary="How replicated sync works">
        <p className="settings-hint">
          Replicates tasks, snippets, Split Inboxes, and account metadata as end-to-end encrypted files through
          folders you choose. There is no Three Strands-operated sync server: ThreeStrands never sees the plaintext,
          but anyone with access to a selected folder can see the encrypted files themselves (their size and timing,
          not their contents). Deleting a folder here removes this device&apos;s copy of the synchronized data from
          that folder; it does not erase copies elsewhere (other devices, cloud provider version history, or other
          configured folders).
        </p>
      </Disclosure>

      {conflicts.length > 0 ? (
        <div>
          <h3>Resolve Conflicts</h3>
          {conflicts.map((conflict) => {
            const key = `conflict:${conflict.entityType}-${conflict.entityId}-${conflict.field}`;
            return (
              <div key={key}>
                <FrontierConflictEditor
                  conflict={conflict}
                  disabled={busy}
                  onResolve={(chosen) => actFor(key, () => replicatedSyncResolveConflict(conflict, chosen))}
                />
                <InlineStatus operation={operation} for={key} />
              </div>
            );
          })}
        </div>
      ) : null}

      {sectionStatus}

      {enrolled ? (
        <EnrolledOverview
          deviceCount={enrolled.deviceCount}
          transports={transports}
          pendingRequests={pendingRequests}
          deviceRoster={deviceRoster}
          operation={operation}
          locations={locations}
          betaToggle={betaToggle}
        />
      ) : (
        <SetupSteps
          step={currentSetupStep(enrollmentStatus, transports.length)}
          enrollmentStatus={enrollmentStatus}
          transports={transports}
          spacePresence={spacePresence}
          operation={operation}
          refresh={refresh}
          beginGenesis={beginGenesis}
          locations={locations}
          betaToggle={betaToggle}
        />
      )}
    </section>
  );
}

function SetupSteps({
  step,
  enrollmentStatus,
  transports,
  spacePresence,
  operation,
  refresh,
  beginGenesis,
  locations,
  betaToggle,
}: {
  step: SetupStep;
  enrollmentStatus: EnrollmentStatus | null;
  transports: ReplicatedSyncTransportStatus[];
  spacePresence: SyncSpacePresence | "checking" | null;
  operation: Operation;
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
        operation={operation}
        refresh={refresh}
        beginGenesis={beginGenesis}
      />
    ),
    verify: <VerifyStep enrollmentStatus={enrollmentStatus} operation={operation} />,
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
                <Disclosure className="sync-setup-step-body" summary={transports.length === 1 ? transports[0]!.location : plural(transports.length, "sync location")}>
                  {locations}
                </Disclosure>
              ) : null}
              {state === "upcoming" && candidate.upcoming ? <p className="settings-hint sync-setup-step-body">{candidate.upcoming}</p> : null}
            </li>
          );
        })}
      </ol>
      <Disclosure summary="Advanced">
        {betaToggle}
      </Disclosure>
    </>
  );
}

function SetupChoice({
  transportCount,
  spacePresence,
  operation,
  refresh,
  beginGenesis,
}: {
  transportCount: number;
  spacePresence: SyncSpacePresence | "checking" | null;
  operation: Operation;
  refresh(): Promise<void>;
  beginGenesis(allowExistingSpace: boolean): void;
}) {
  const { busy, runFor, actFor } = operation;
  const [recoveryPhraseInput, setRecoveryPhraseInput] = useState("");
  const [phraseCheck, setPhraseCheck] = useState<RecoveryPhraseCheck | null>(null);
  const [confirmingSeparateSpace, setConfirmingSeparateSpace] = useState(false);
  const feedbackId = useId();
  const noLocation = transportCount === 0;

  useEffect(() => {
    if (!recoveryPhraseInput.trim()) {
      setPhraseCheck(null);
      return;
    }
    let cancelled = false;
    replicatedSyncCheckRecoveryPhrase(recoveryPhraseInput).then(
      (check) => { if (!cancelled) setPhraseCheck(check); },
      () => { if (!cancelled) setPhraseCheck(null); },
    );
    return () => { cancelled = true; };
  }, [recoveryPhraseInput]);
  const feedback = recoveryPhraseFeedback(recoveryPhraseInput, phraseCheck);

  const joinWithPhrase = () => runFor("join-phrase", async () => {
    await replicatedSyncJoinWithRecoveryPhrase(recoveryPhraseInput.trim().split(/\s+/).join(" "));
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
        <>
          <button
            type="button"
            className="primary-action"
            disabled={busy || noLocation || spacePresence === "checking"}
            onClick={() => beginGenesis(false)}
          >
            Create a new sync space
          </button>
          <InlineStatus operation={operation} for="genesis" />
        </>
      )}
      <button
        type="button"
        className={spacePresence === "existing" ? "primary-action" : "account-action-button"}
        disabled={busy || noLocation}
        onClick={() => actFor("join-request", replicatedSyncRequestEnrollment)}
      >
        Request to join from an existing device
      </button>
      <InlineStatus operation={operation} for="join-request" />
      <label className="settings-field">
        <span>Or join with a recovery phrase</span>
        <textarea
          rows={3}
          placeholder="24 words separated by spaces"
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          value={recoveryPhraseInput}
          disabled={busy}
          aria-describedby={feedback ? feedbackId : undefined}
          aria-invalid={feedback?.tone === "attention" ? true : undefined}
          onChange={(event) => setRecoveryPhraseInput(event.target.value)}
        />
      </label>
      {feedback ? (
        <p id={feedbackId} className={`settings-hint recovery-phrase-feedback recovery-phrase-feedback-${feedback.tone}`} aria-live="polite">
          {feedback.text}
        </p>
      ) : null}
      <button
        type="button"
        className="account-action-button"
        disabled={busy || !phraseCheck?.valid || noLocation}
        onClick={joinWithPhrase}
      >
        Join with recovery phrase
      </button>
      <InlineStatus operation={operation} for="join-phrase" />
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
          <>
            <button type="button" className="account-action-button" disabled={busy} onClick={() => setConfirmingSeparateSpace(true)}>
              Create a separate sync space instead…
            </button>
            <InlineStatus operation={operation} for="genesis" />
          </>
        )
      ) : null}
    </>
  );
}

function VerifyStep({ enrollmentStatus, operation }: { enrollmentStatus: EnrollmentStatus | null; operation: Operation }) {
  const { busy, actFor } = operation;
  const cancel = (
    <LeaveControl
      operation={operation}
      trigger="Cancel and start over…"
      title="Cancel this request?"
      body="This device forgets its pending request and returns to the start of setup. Nothing has synced yet, and your sync locations stay configured."
      confirm="Cancel request"
    />
  );
  if (enrollmentStatus?.state === "awaitingGrant") {
    return (
      <>
        <p className="settings-hint">
          Open Replicated Sync on one of your existing devices and approve this device there. It will show you a code
          to compare with this one:
        </p>
        <p className="sync-fingerprint">{enrollmentStatus.fingerprint}</p>
        <button type="button" className="account-action-button" disabled={busy} onClick={() => actFor("check-approval", replicatedSyncNow)}>
          Check for approval
        </button>
        <InlineStatus operation={operation} for="check-approval" />
        {cancel}
      </>
    );
  }
  if (enrollmentStatus?.state === "awaitingConfirmation") {
    const requestId = enrollmentStatus.requestId;
    return (
      <>
        <p className="settings-hint">
          An approval arrived. Compare these codes with what the approving device shows. They must match exactly
          before you confirm.
        </p>
        <p className="sync-fingerprint">This device: {enrollmentStatus.fingerprint}</p>
        <p className="sync-fingerprint">Approver: {enrollmentStatus.approverFingerprint}</p>
        <button type="button" className="primary-action" disabled={busy} onClick={() => actFor("confirm", () => replicatedSyncConfirmEnrollment(requestId))}>
          Confirm — fingerprints match
        </button>
        <InlineStatus operation={operation} for="confirm" />
        {cancel}
      </>
    );
  }
  return null;
}

/** Leaves the sync space (or abandons a pending join) on this device only. */
function LeaveControl({
  operation,
  trigger,
  title,
  body,
  confirm,
}: {
  operation: Operation;
  trigger: string;
  title: string;
  body: string;
  confirm: string;
}) {
  const { busy, actFor } = operation;
  const [confirming, setConfirming] = useState(false);
  return (
    <>
      {confirming ? (
        <div className="settings-inline-confirm" role="group" aria-label={title}>
          <p><strong>{title}</strong><br />{body}</p>
          <span className="settings-inline-confirm-actions">
            <button type="button" disabled={busy} onClick={() => setConfirming(false)}>Keep</button>
            <button type="button" className="danger-action" disabled={busy} onClick={() => { setConfirming(false); actFor("leave", replicatedSyncLeave); }}>
              {confirm}
            </button>
          </span>
        </div>
      ) : (
        <button type="button" className="account-action-button danger-action" disabled={busy} onClick={() => setConfirming(true)}>
          {trigger}
        </button>
      )}
      <InlineStatus operation={operation} for="leave" />
    </>
  );
}

function PendingRequestCard({ request, operation }: { request: IncomingEnrollmentRequest; operation: Operation }) {
  const { busy, actFor } = operation;
  const [reviewing, setReviewing] = useState(false);
  const [name, setName] = useState("");
  const key = `request:${request.requestId}`;
  const deviceId = request.deviceId;

  const approve = () => actFor(key, async () => {
    await replicatedSyncApproveRequest(request.requestId);
    if (deviceId && name.trim()) await replicatedSyncSetDeviceLabel(deviceId, name);
  });

  return (
    <li className="account-card">
      <div className="account-card-row">
        <div className="account-card-identity">
          <strong>New device asking to join</strong>
          <span className="account-card-email">Requested {new Date(request.createdAt).toLocaleString()}</span>
        </div>
        {reviewing ? null : (
          <button type="button" className="primary-action" disabled={busy} aria-expanded={false} onClick={() => setReviewing(true)}>
            Review…
          </button>
        )}
        <button type="button" className="account-action-button danger-action" disabled={busy} onClick={() => actFor(key, () => replicatedSyncRejectRequest(request.requestId))}>
          Reject
        </button>
      </div>
      {reviewing ? (
        <div className="settings-inline-panel" role="group" aria-label="Approve device confirmation">
          <p>On the new device, check that Replicated Sync shows exactly this code:</p>
          <p className="sync-fingerprint">{request.fingerprint}</p>
          <p className="settings-hint">If the codes don’t match, reject the request. Someone else may be trying to join.</p>
          {deviceId ? (
            <label className="settings-field">
              <span>Name this device (optional, only shown on this device)</span>
              <input type="text" maxLength={MAX_DEVICE_LABEL_CHARS} value={name} disabled={busy} onChange={(event) => setName(event.target.value)} />
            </label>
          ) : null}
          <span className="settings-inline-confirm-actions">
            <button type="button" disabled={busy} onClick={() => setReviewing(false)}>Cancel</button>
            <button type="button" className="primary-action" disabled={busy} onClick={approve}>Codes match — approve</button>
          </span>
        </div>
      ) : null}
      <InlineStatus operation={operation} for={key} />
    </li>
  );
}

function DeviceCard({ device, operation }: { device: DeviceRosterEntry; operation: Operation }) {
  const { busy, actFor } = operation;
  const [revoking, setRevoking] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState(device.label ?? "");
  const key = `device:${device.deviceId}`;
  const details = [
    device.isSelf && device.label ? "This device" : null,
    device.status === "revoked" ? "Revoked" : "Active",
    device.lastChangeAt ? `Last change ${new Date(device.lastChangeAt).toLocaleString()}` : "No changes yet",
    `ID ${device.deviceId.slice(0, 8)}`,
  ].filter(Boolean).join(" · ");

  return (
    <li className="account-card">
      <div className="account-card-row">
        <div className="account-card-identity">
          <strong>{deviceDisplayName(device)}</strong>
          <span className="account-card-email">{details}</span>
        </div>
        {renaming ? null : (
          <button type="button" className="account-action-button" disabled={busy} onClick={() => { setName(device.label ?? ""); setRenaming(true); }}>
            {device.label ? "Rename" : "Name"}
          </button>
        )}
        {!device.isSelf && device.status === "active" ? (
          <button type="button" className="account-action-button danger-action" disabled={busy} aria-expanded={revoking} onClick={() => setRevoking(true)}>
            Revoke…
          </button>
        ) : null}
      </div>
      {renaming ? (
        <form
          className="settings-inline-panel"
          aria-label="Device name"
          onSubmit={(event) => {
            event.preventDefault();
            setRenaming(false);
            actFor(key, () => replicatedSyncSetDeviceLabel(device.deviceId, name));
          }}
        >
          <label className="settings-field">
            <span>Name (only shown on this device)</span>
            <input type="text" maxLength={MAX_DEVICE_LABEL_CHARS} value={name} disabled={busy} autoFocus onChange={(event) => setName(event.target.value)} />
          </label>
          <span className="settings-inline-confirm-actions">
            <button type="button" disabled={busy} onClick={() => setRenaming(false)}>Cancel</button>
            <button type="submit" className="primary-action" disabled={busy}>Save</button>
          </span>
        </form>
      ) : null}
      {revoking ? (
        <div className="settings-inline-confirm" role="group" aria-label="Revoke device confirmation">
          <p><strong>Revoke this device?</strong><br />It keeps existing data, but future writes from it will no longer be trusted.</p>
          <span className="settings-inline-confirm-actions">
            <button type="button" disabled={busy} onClick={() => setRevoking(false)}>Cancel</button>
            <button type="button" className="danger-action" disabled={busy} onClick={() => { setRevoking(false); actFor(key, () => replicatedSyncRotateEpoch(device.deviceId)); }}>Revoke device</button>
          </span>
        </div>
      ) : null}
      <InlineStatus operation={operation} for={key} />
    </li>
  );
}

function EnrolledOverview({
  deviceCount,
  transports,
  pendingRequests,
  deviceRoster,
  operation,
  locations,
  betaToggle,
}: {
  deviceCount: number;
  transports: ReplicatedSyncTransportStatus[];
  pendingRequests: IncomingEnrollmentRequest[];
  deviceRoster: DeviceRosterEntry[];
  operation: Operation;
  locations: ReactNode;
  betaToggle: ReactNode;
}) {
  const { busy, actFor } = operation;
  const overview = syncOverview(transports, deviceCount);

  return (
    <>
      <div className={`sync-overview sync-overview-${overview.tone}`} role="status" aria-label="Sync status">
        <span>{overview.text}</span>
        <button type="button" className="account-action-button" disabled={busy || transports.length === 0} onClick={() => actFor("sync-now", replicatedSyncNow)}>
          Sync now
        </button>
      </div>
      <InlineStatus operation={operation} for="sync-now" />

      {pendingRequests.length > 0 ? (
        <>
          <h4>Devices waiting to join</h4>
          <ul className="accounts-list" aria-label="Devices waiting to join">
            {pendingRequests.map((request) => <PendingRequestCard key={request.requestId} request={request} operation={operation} />)}
          </ul>
        </>
      ) : null}

      <Disclosure summary={`Devices (${deviceRoster.length})`} open>
        <ul className="accounts-list" aria-label="Devices">
          {deviceRoster.map((device) => <DeviceCard key={device.deviceId} device={device} operation={operation} />)}
        </ul>
      </Disclosure>

      <Disclosure summary={`Sync locations (${transports.length})`} open={overview.tone === "attention"}>
        {locations}
      </Disclosure>

      <Disclosure summary="Advanced">
        {betaToggle}
        <LeaveControl
          operation={operation}
          trigger="Leave this sync space…"
          title="Leave this sync space?"
          body="This device stops syncing and forgets its keys for this space. Tasks, snippets, and other data stay on this device, and your sync locations stay configured so you can rejoin later. Changes that haven’t synced yet won’t reach your other devices, and they will keep listing this device until you revoke it from one of them."
          confirm="Leave sync space"
        />
      </Disclosure>
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
  const { busy, setError, runFor, actFor } = operation;
  const [ipfsBaseUrl, setIpfsBaseUrl] = useState("");
  const [ipfsToken, setIpfsToken] = useState("");
  const [ipfsProbe, setIpfsProbe] = useState<IpfsRpcProbeReport | null>(null);
  const [disconnectingTransport, setDisconnectingTransport] = useState<string | null>(null);

  const addFolder = () => runFor("add-folder", async () => {
    const status = await replicatedSyncAddFolder();
    if (!status) setError("No folder selected.", "add-folder");
    await refresh();
  });

  const probeIpfsRpc = () => {
    setIpfsProbe(null);
    runFor("ipfs", async () => {
      const report = await replicatedSyncProbeIpfsRpc(ipfsBaseUrl, ipfsToken.trim() ? ipfsToken : null);
      setIpfsProbe(report);
      if (!report.versionOk) setError("Could not reach an IPFS RPC endpoint at that URL.", "ipfs");
    });
  };

  const addIpfsRpc = () => runFor("ipfs", async () => {
    const status = await replicatedSyncAddIpfsRpc(ipfsBaseUrl, ipfsToken.trim() ? ipfsToken : null);
    if (!status) setError("Could not add that endpoint.", "ipfs");
    setIpfsBaseUrl("");
    setIpfsToken("");
    setIpfsProbe(null);
    await refresh();
  });

  return (
    <>
      <p className="settings-hint">
        Every device in a sync space must use the same location. Choose a folder your devices already share, or use
        one dedicated Filebase bucket for the sync space. Do not reuse that bucket for a separate sync space.
      </p>
      {transports.length > 0 ? (
        <ul className="accounts-list" aria-label="Sync locations">
          {transports.map((transport) => {
            const health = describeTransportHealth(transport);
            const key = `transport:${transport.instanceId}`;
            return (
              <li className={`account-card sync-location-${health.tone}`} key={transport.instanceId}>
                <div className="account-card-row">
                  <div className="account-card-identity">
                    <strong>{transport.location}</strong>
                    <span className="account-card-email">
                      {transport.kind === "ipfs_rpc" ? "IPFS RPC" : "Folder"} · <span className="sync-location-health">{health.label}</span>
                      {transport.storageBytes != null ? ` · ${formatStorageEstimate(transport.storageBytes)}` : ""}
                    </span>
                    {health.detail ? <span className="account-card-email">{health.detail}</span> : null}
                    {!transport.headDiscovery ? (
                      <span className="account-card-email">Storage only: other devices can’t discover new changes through this location on its own.</span>
                    ) : null}
                    {transport.lastSuccessAt ? (
                      <span className="account-card-email">Last synced {new Date(transport.lastSuccessAt).toLocaleString()}</span>
                    ) : null}
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
                      <button type="button" disabled={busy} onClick={() => { setDisconnectingTransport(null); actFor(key, () => replicatedSyncRemoveTransport(transport.instanceId, false)); }}>Disconnect and keep data</button>
                      {transport.kind !== "ipfs_rpc" ? (
                        <button type="button" className="danger-action" disabled={busy} onClick={() => { setDisconnectingTransport(null); actFor(key, () => replicatedSyncRemoveTransport(transport.instanceId, true)); }}>Delete files and disconnect</button>
                      ) : null}
                    </span>
                  </div>
                ) : null}
                <InlineStatus operation={operation} for={key} />
              </li>
            );
          })}
        </ul>
      ) : null}

      <button type="button" className={transports.length === 0 ? "primary-action" : "account-action-button"} disabled={busy} onClick={addFolder}>
        {transports.length === 0 ? "Add a sync folder" : "Add another sync folder"}
      </button>
      <InlineStatus operation={operation} for="add-folder" />

      <Disclosure summary="Use an IPFS RPC endpoint instead">
        <p className="settings-hint">
          Advanced: point at a Kubo-compatible RPC endpoint or a dedicated Filebase bucket. For Filebase, create one
          bucket for this sync space, generate its bucket-specific RPC token, and enter that same token on every
          device joining the space. The token is stored only in this device&apos;s OS keychain, never in a settings
          export.
        </p>
        <label className="settings-field">
          <span>RPC base URL</span>
          <input
            type="text"
            placeholder="https://ipfs.example.com:5001"
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
        <div className="settings-row">
          <button
            type="button"
            className="account-action-button"
            disabled={busy}
            onClick={() => {
              setIpfsBaseUrl(FILEBASE_RPC_URL);
              setIpfsProbe(null);
            }}
          >
            Fill in Filebase URL
          </button>
          <button type="button" className="account-action-button" disabled={busy || !ipfsBaseUrl} onClick={probeIpfsRpc}>
            Test connection
          </button>
        </div>
        {ipfsProbe?.versionOk ? (
          <p className="settings-hint">
            {`Reachable · ${ipfsProbe.headDiscoveryAvailable ? "supports sync discovery through bucket pins" : "bucket pins unavailable"}`}
          </p>
        ) : null}
        <button type="button" className="primary-action" disabled={busy || !ipfsBaseUrl} onClick={addIpfsRpc}>
          Add IPFS RPC endpoint
        </button>
        <InlineStatus operation={operation} for="ipfs" />
      </Disclosure>
    </>
  );
}
