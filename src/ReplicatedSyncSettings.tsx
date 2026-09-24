import { useCallback, useEffect, useId, useState, type ReactNode } from "react";
import { errorMessage } from "./errors";
import { FrontierConflictEditor } from "./FrontierConflictEditor";
import { AddDevicePanel, JoinCodeNotices, JoinCodePanel, OutstandingJoinCodes } from "./JoinCodePanels";
import { holdRecoveryPhrase } from "./RecoveryPhraseDialog";
import {
  MAX_DEVICE_LABEL_CHARS,
  replicatedSyncApproveRequest,
  replicatedSyncBeginGenesis,
  replicatedSyncBetaEnabled,
  replicatedSyncCheckRecoveryPhrase,
  replicatedSyncConfirmEnrollment,
  replicatedSyncConflicts,
  replicatedSyncDeviceRoster,
  replicatedSyncDismissProtocolResetNotice,
  replicatedSyncEnabled,
  replicatedSyncEnrollmentStatus,
  replicatedSyncInspectSpace,
  replicatedSyncJoinCodeNotices,
  replicatedSyncJoinWithRecoveryPhrase,
  replicatedSyncLeave,
  replicatedSyncListJoinCodes,
  replicatedSyncNow,
  replicatedSyncPendingRequests,
  replicatedSyncProtocolResetNotice,
  replicatedSyncRejectRequest,
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
  type JoinCodeNotice,
  type OutstandingJoinCode,
  type RecoveryPhraseCheck,
  type ReplicatedSyncTransportStatus,
  type SyncSpacePresence,
} from "./replicatedSync";
import { connectorDisplayName, ConnectorList } from "./SyncConnectors";
import { queuePortablePreferencesAndWait } from "./syncedPreferences";
import { ANY_OPERATION, useLiveStatus, useSettingsOperation } from "./settingsOperations";
import { describeTransportHealth, Disclosure, InlineStatus, plural, type Operation, type Tone } from "./syncSettingsParts";

export { describeTransportHealth, type Tone, type TransportHealthSummary } from "./syncSettingsParts";

const RECOVERY_PHRASE_WORDS = 24;

function listPositions(positions: number[]): string {
  const numbers = positions.map((position) => String(position + 1));
  if (numbers.length <= 1) return numbers.join("");
  return `${numbers.slice(0, -1).join(", ")} and ${numbers.at(-1)}`;
}

export type SetupStep = "location" | "choose" | "verify";

export const SETUP_STEPS: readonly { id: SetupStep; title: string; upcoming: string }[] = [
  { id: "location", title: "Add a connector", upcoming: "" },
  {
    id: "choose",
    title: "Join your sync group",
    upcoming: "Next, join the sync group your other devices use, or start a new one if this is your first device.",
  },
  {
    id: "verify",
    title: "Verify this device",
    upcoming: "If another device approves this one, you’ll compare a short code on both screens before syncing starts.",
  },
];

/** Which setup step a not-yet-enrolled device is on. Enrollment progress
 * wins over connectors: a device mid-verification stays there even if its
 * connectors change underneath it. */
export function currentSetupStep(status: EnrollmentStatus | null, transportCount: number): SetupStep {
  if (status?.state === "awaitingGrant" || status?.state === "awaitingConfirmation" || status?.state === "rejected") return "verify";
  return transportCount === 0 ? "location" : "choose";
}

export type SyncOverview = { tone: Tone; text: string };

/** One-line health summary for an enrolled device. */
export function syncOverview(transports: readonly ReplicatedSyncTransportStatus[], deviceCount: number): SyncOverview {
  const devices = plural(deviceCount, "device");
  if (transports.length === 0) {
    return { tone: "attention", text: `Not syncing · ${devices} · this device has no connectors. Add one under Connectors.` };
  }
  const troubled = transports.filter((transport) => describeTransportHealth(transport).tone === "attention").length;
  if (troubled > 0) {
    return { tone: "attention", text: `${devices} · ${plural(troubled, "connector")} ${troubled === 1 ? "needs" : "need"} attention` };
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

export function ReplicatedSyncSettings() {
  const [available, setAvailable] = useState<boolean | null>(null);
  const [betaEnabled, setBetaEnabledState] = useState(false);
  const [transports, setTransports] = useState<ReplicatedSyncTransportStatus[]>([]);
  const [conflicts, setConflicts] = useState<FrontierConflict[]>([]);
  const [enrollmentStatus, setEnrollmentStatus] = useState<EnrollmentStatus | null>(null);
  const [pendingRequests, setPendingRequests] = useState<IncomingEnrollmentRequest[]>([]);
  const [deviceRoster, setDeviceRoster] = useState<DeviceRosterEntry[]>([]);
  const [joinCodes, setJoinCodes] = useState<OutstandingJoinCode[]>([]);
  const [joinNotices, setJoinNotices] = useState<JoinCodeNotice[]>([]);
  const [spacePresence, setSpacePresence] = useState<SyncSpacePresence | "checking" | null>(null);
  const [presenceCheck, setPresenceCheck] = useState(0);
  const [protocolResetNotice, setProtocolResetNotice] = useState(false);

  const refresh = useCallback(async () => {
    const [enabled, beta] = await Promise.all([replicatedSyncEnabled(), replicatedSyncBetaEnabled()]);
    setAvailable(enabled);
    setBetaEnabledState(beta);
    if (enabled) {
      const [nextTransports, nextConflicts, nextStatus, nextPending, nextRoster, nextCodes, nextNotices, nextResetNotice] = await Promise.all([
        replicatedSyncStatus(),
        replicatedSyncConflicts(),
        replicatedSyncEnrollmentStatus(),
        replicatedSyncPendingRequests(),
        replicatedSyncDeviceRoster(),
        replicatedSyncListJoinCodes(),
        replicatedSyncJoinCodeNotices(),
        replicatedSyncProtocolResetNotice(),
      ]);
      setProtocolResetNotice(nextResetNotice === true);
      setTransports(nextTransports);
      setConflicts(nextConflicts);
      setEnrollmentStatus(nextStatus);
      setPendingRequests(nextPending);
      setDeviceRoster(nextRoster);
      setJoinCodes(nextCodes ?? []);
      setJoinNotices(nextNotices ?? []);
    } else {
      setTransports([]);
      setConflicts([]);
      setEnrollmentStatus(null);
      setPendingRequests([]);
      setDeviceRoster([]);
      setJoinCodes([]);
      setJoinNotices([]);
      setProtocolResetNotice(false);
    }
  }, []);

  const operation = useSettingsOperation(refresh);
  const { busy, setError, runFor, actFor } = operation;
  const reportError = useCallback((reason: unknown) => setError(errorMessage(reason)), [setError]);
  useLiveStatus("replicated-sync-status", refresh, reportError);

  // Before this device joins or starts a group, look for one that another
  // device already published to the configured connectors, so the setup
  // choice can steer toward joining it instead of forking a second group.
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
      // A refusal means another device published a group since the last
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
          Strands-operated server: it replicates directly through a shared folder, S3-compatible storage, or an IPFS
          endpoint you choose. Turn it on to set it up on this device.
        </p>
        {betaToggle}
        {sectionStatus}
      </section>
    );
  }

  const connectors = <ConnectorList transports={transports} operation={operation} refresh={refresh} />;
  const enrolled = enrollmentStatus?.state === "enrolled" ? enrollmentStatus : null;

  return (
    <section className="settings-section" aria-label="Replicated Sync">
      <h3>Replicated Sync (Beta)</h3>
      <Disclosure summary="How replicated sync works">
        <p className="settings-hint">
          Replicates tasks, snippets, Split Inboxes, and account metadata as end-to-end encrypted files through
          connectors you choose: a shared folder, an S3-compatible bucket, or an IPFS endpoint. There is no Three
          Strands-operated sync server: ThreeStrands never sees the plaintext, but anyone with access to a connector’s
          storage can see the encrypted files themselves (their size and timing, not their contents). Deleting a
          connector’s files here removes this device&apos;s copy of the synchronized data from that storage; it does
          not erase copies elsewhere (other devices, provider version history, or other connectors).
        </p>
      </Disclosure>

      {protocolResetNotice ? (
        <div className="account-card sync-notice" role="status">
          <div className="account-card-row">
            <p className="account-card-identity">
              This update reset sync on this device: sync groups made by earlier test versions can’t be used any more.
              Your data here is kept. Update every device, create a new sync group on one of them, then join it from
              the others.
            </p>
            <button
              type="button"
              className="account-action-button"
              disabled={busy}
              onClick={() => actFor("protocol-reset-notice", replicatedSyncDismissProtocolResetNotice)}
            >
              Dismiss
            </button>
          </div>
          <InlineStatus operation={operation} for="protocol-reset-notice" />
        </div>
      ) : null}

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
          awaitingAdmissionFrom={enrolled.awaitingAdmissionFrom ?? null}
          transports={transports}
          pendingRequests={pendingRequests}
          deviceRoster={deviceRoster}
          joinCodes={joinCodes}
          joinNotices={joinNotices}
          operation={operation}
          refresh={refresh}
          connectors={connectors}
          betaToggle={betaToggle}
        />
      ) : (
        <SetupScreen
          step={currentSetupStep(enrollmentStatus, transports.length)}
          enrollmentStatus={enrollmentStatus}
          transports={transports}
          spacePresence={spacePresence}
          operation={operation}
          refresh={refresh}
          beginGenesis={beginGenesis}
          connectors={connectors}
          betaToggle={betaToggle}
        />
      )}
    </section>
  );
}

function SetupScreen({
  step,
  enrollmentStatus,
  transports,
  spacePresence,
  operation,
  refresh,
  beginGenesis,
  connectors,
  betaToggle,
}: {
  step: SetupStep;
  enrollmentStatus: EnrollmentStatus | null;
  transports: ReplicatedSyncTransportStatus[];
  spacePresence: SyncSpacePresence | "checking" | null;
  operation: Operation;
  refresh(): Promise<void>;
  beginGenesis(allowExistingSpace: boolean): void;
  connectors: ReactNode;
  betaToggle: ReactNode;
}) {
  const joinCodeInputId = useId();
  const currentIndex = SETUP_STEPS.findIndex((candidate) => candidate.id === step);
  const offerJoinCode = step !== "verify";
  const bodies: Record<SetupStep, ReactNode> = {
    location: connectors,
    choose: (
      <SetupChoice
        transportCount={transports.length}
        spacePresence={spacePresence}
        operation={operation}
        refresh={refresh}
        beginGenesis={beginGenesis}
        joinCodeInputId={offerJoinCode ? joinCodeInputId : null}
      />
    ),
    verify: <VerifyStep enrollmentStatus={enrollmentStatus} operation={operation} />,
  };

  return (
    <>
      {offerJoinCode ? (
        <div className="sync-join-code-panel" role="group" aria-label="Join with a code from another device">
          <h4>Join with a code from another device</h4>
          <p className="settings-hint">
            On a device that already syncs, open Replicated Sync, choose Devices → Add a device, then paste the code here.
          </p>
          <JoinCodePanel operation={operation} refresh={refresh} inputId={joinCodeInputId} />
        </div>
      ) : null}
      {offerJoinCode ? (
        <p className="settings-hint sync-setup-divider">Or set up manually — for your first device, or when no other device is at hand.</p>
      ) : null}
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
                <Disclosure
                  className="sync-setup-step-body"
                  summary={transports.length === 1 ? connectorDisplayName(transports[0]!) : transports.map(connectorDisplayName).join(", ")}
                >
                  {connectors}
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
  joinCodeInputId,
}: {
  transportCount: number;
  spacePresence: SyncSpacePresence | "checking" | null;
  operation: Operation;
  refresh(): Promise<void>;
  beginGenesis(allowExistingSpace: boolean): void;
  joinCodeInputId: string | null;
}) {
  const { busy, runFor, actFor } = operation;
  const [recoveryPhraseInput, setRecoveryPhraseInput] = useState("");
  const [phraseCheck, setPhraseCheck] = useState<RecoveryPhraseCheck | null>(null);
  const [confirmingSeparateSpace, setConfirmingSeparateSpace] = useState(false);
  const feedbackId = useId();
  const noConnector = transportCount === 0;
  const existing = spacePresence === "existing";
  const legacy = spacePresence === "legacy";

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

  const createButton = existing ? null : (
    <>
      <button
        type="button"
        className="primary-action"
        disabled={busy || noConnector || spacePresence === "checking"}
        onClick={() => beginGenesis(false)}
      >
        Create a new sync group
      </button>
      <InlineStatus operation={operation} for="genesis" />
    </>
  );

  const recoveryPhraseEntry = (
    <>
      <label className="settings-field">
        <span>{existing ? "Enter the recovery phrase" : "Or join with a recovery phrase"}</span>
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
        className={existing ? "primary-action" : "account-action-button"}
        disabled={busy || !phraseCheck?.valid || noConnector}
        onClick={joinWithPhrase}
      >
        Join with recovery phrase
      </button>
      <InlineStatus operation={operation} for="join-phrase" />
    </>
  );

  if (legacy) {
    return (
      <p className="settings-hint" role="status">
        This connector holds a sync group from an earlier test version of Three Strands, which this version can’t use
        — or a shared folder hasn’t finished syncing yet. If you just set up sync on another device, wait for your sync
        app to finish, then reopen this page. Otherwise update every device, delete this connector’s files (Delete files and
        disconnect, under its settings), add it again, and create a new sync group.
      </p>
    );
  }

  return (
    <>
      <p className="settings-hint" role="status">
        {spacePresence === "checking"
          ? "Checking your connectors for an existing sync group…"
          : existing
            ? "Another device already set up a sync group here. Join it to share data with your other devices."
            : spacePresence === "none"
              ? "No sync group found here yet. If this is your first device, create one. On your other devices, add this same connector, then join."
              : spacePresence === "unknown"
                ? "Couldn’t check every connector for an existing sync group. If another device already syncs here, join it rather than creating a new one."
                : "Set this device up as the first device in a new encrypted sync group, or join a group that already exists on another device."}
      </p>
      {createButton}
      {existing ? recoveryPhraseEntry : null}
      <button
        type="button"
        className="account-action-button"
        disabled={busy || noConnector}
        onClick={() => actFor("join-request", replicatedSyncRequestEnrollment)}
      >
        Ask another device to approve this one
      </button>
      <InlineStatus operation={operation} for="join-request" />
      {existing ? null : recoveryPhraseEntry}
      {existing && joinCodeInputId ? (
        <p className="settings-hint">
          Have another device nearby? A join code from it is faster.{" "}
          <button type="button" className="link-button" onClick={() => document.getElementById(joinCodeInputId)?.focus()}>
            Paste a join code
          </button>
        </p>
      ) : null}
      {existing ? (
        confirmingSeparateSpace ? (
          <div className="settings-inline-confirm" role="group" aria-label="Create a separate sync group confirmation">
            <p>
              <strong>Create a separate sync group?</strong><br />
              This device will not sync with the devices already using this connector, and the new group gets its own
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
                Create separate group
              </button>
            </span>
          </div>
        ) : (
          <>
            <button type="button" className="account-action-button" disabled={busy} onClick={() => setConfirmingSeparateSpace(true)}>
              Create a separate sync group instead…
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
      body="This device forgets its pending request and returns to the start of setup. Nothing has synced yet, and your connectors stay configured."
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
  if (enrollmentStatus?.state === "rejected") {
    return (
      <>
        <p className="settings-inline-status">A rejection response arrived. Check with your group before starting a new request.</p>
        <LeaveControl
          operation={operation}
          trigger="Start a new request…"
          title="Start a new request?"
          body="This device forgets the rejected request and returns to the start of setup. Your connectors stay configured."
          confirm="Start over"
        />
      </>
    );
  }
  return null;
}

/** Leaves the sync group (or abandons a pending join) on this device only. */
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
  const [rejecting, setRejecting] = useState(false);
  const [name, setName] = useState("");
  const key = `request:${request.requestId}`;
  const deviceId = request.deviceId;

  const approve = () => actFor(key, async () => {
    await replicatedSyncApproveRequest(request.requestId);
    if (deviceId && name.trim()) {
      await queuePortablePreferencesAndWait();
      await replicatedSyncSetDeviceLabel(deviceId, name);
    }
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
        {rejecting ? null : (
          <button type="button" className="account-action-button danger-action" disabled={busy} onClick={() => setRejecting(true)}>
            Reject…
          </button>
        )}
      </div>
      {rejecting ? (
        <div className="settings-inline-confirm" role="group" aria-label="Reject device request confirmation">
          <p>Reject this request on all your sync devices? If an approval grant has already been published, that approval takes precedence.</p>
          <span className="settings-inline-confirm-actions">
            <button type="button" disabled={busy} onClick={() => setRejecting(false)}>Keep pending</button>
            <button type="button" className="danger-action" disabled={busy} onClick={() => { setRejecting(false); actFor(key, () => replicatedSyncRejectRequest(request.requestId)); }}>
              Reject on all devices
            </button>
          </span>
        </div>
      ) : null}
      {reviewing ? (
        <div className="settings-inline-panel" role="group" aria-label="Approve device confirmation">
          <p>On the new device, check that Replicated Sync shows exactly this code:</p>
          <p className="sync-fingerprint">{request.fingerprint}</p>
          <p className="settings-hint">If the codes don’t match, reject the request. Someone else may be trying to join.</p>
          {deviceId ? (
            <label className="settings-field">
              <span>Name this device (shared with your other devices)</span>
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
    device.isSelf ? "This device" : null,
    device.status === "revoked" ? "Revoked" : "Active",
    device.joinedWithJoinCode ? "Joined with a join code" : null,
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
            actFor(key, async () => {
              await queuePortablePreferencesAndWait();
              await replicatedSyncSetDeviceLabel(device.deviceId, name);
            });
          }}
        >
          <label className="settings-field">
            <span>Name (shared with your other devices)</span>
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
  awaitingAdmissionFrom,
  transports,
  pendingRequests,
  deviceRoster,
  joinCodes,
  joinNotices,
  operation,
  refresh,
  connectors,
  betaToggle,
}: {
  deviceCount: number;
  awaitingAdmissionFrom: string | null;
  transports: ReplicatedSyncTransportStatus[];
  pendingRequests: IncomingEnrollmentRequest[];
  deviceRoster: DeviceRosterEntry[];
  joinCodes: OutstandingJoinCode[];
  joinNotices: JoinCodeNotice[];
  operation: Operation;
  refresh(): Promise<void>;
  connectors: ReactNode;
  betaToggle: ReactNode;
}) {
  const { busy, actFor } = operation;
  const [addingDevice, setAddingDevice] = useState(false);
  const overview = syncOverview(transports, deviceCount);

  return (
    <>
      {awaitingAdmissionFrom ? (
        <div className="sync-overview sync-overview-attention" role="status" aria-label="Joining">
          <span>
            Waiting for {awaitingAdmissionFrom} to finish adding this device. This happens automatically the next time
            it syncs. Until then, your changes here reach your other devices only after it does.
          </span>
        </div>
      ) : null}
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
        <JoinCodeNotices notices={joinNotices} operation={operation} />
        <ul className="accounts-list" aria-label="Devices">
          {deviceRoster.map((device) => <DeviceCard key={device.deviceId} device={device} operation={operation} />)}
        </ul>
        {addingDevice ? (
          <AddDevicePanel transports={transports} operation={operation} refresh={refresh} onClose={() => setAddingDevice(false)} />
        ) : (
          <button type="button" className="account-action-button" disabled={busy || transports.length === 0} onClick={() => setAddingDevice(true)}>
            Add a device
          </button>
        )}
        <OutstandingJoinCodes codes={joinCodes} operation={operation} />
      </Disclosure>

      <Disclosure summary={`Connectors (${transports.length})`} open={overview.tone === "attention"}>
        {connectors}
      </Disclosure>

      <Disclosure summary="Advanced">
        {betaToggle}
        <LeaveControl
          operation={operation}
          trigger="Leave this sync group…"
          title="Leave this sync group?"
          body="This device stops syncing and forgets its keys for this group. Tasks, snippets, and other data stay on this device, and your connectors stay configured so you can rejoin later. Changes that haven’t synced yet won’t reach your other devices, and they will keep listing this device until you revoke it from one of them."
          confirm="Leave sync group"
        />
      </Disclosure>
    </>
  );
}
