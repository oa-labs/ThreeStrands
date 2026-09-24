import { useId, useState, type ReactNode } from "react";
import {
  replicatedSyncAddFolder,
  replicatedSyncAddIpfsRpc,
  replicatedSyncAddS3,
  replicatedSyncProbeIpfsRpc,
  replicatedSyncProbeS3,
  replicatedSyncRemoveTransport,
  replicatedSyncUpdateConnector,
  type ConnectorKind,
  type IpfsRpcProbeReport,
  type ReplicatedSyncTransportStatus,
  type S3ConnectionTest,
  type S3ConnectorConfig,
  type S3Credentials,
} from "./replicatedSync";
import { describeTransportHealth, Disclosure, formatStorageEstimate, InlineStatus, type Operation } from "./syncSettingsParts";
import { InlineConfirm } from "./InlineConfirm";

const FILEBASE_RPC_URL = "https://rpc.filebase.io";
/** Mirrors `MAX_CONNECTOR_LABEL_CHARS` in `sync_connectors.rs`, which enforces it. */
export const MAX_CONNECTOR_LABEL_CHARS = 60;

export function connectorKindLabel(kind: ConnectorKind | string): string {
  switch (kind) {
    case "folder": return "Shared folder";
    case "s3": return "S3 storage";
    case "ipfs_rpc": return "IPFS";
    default: return kind;
  }
}

export function connectorDisplayName(transport: Pick<ReplicatedSyncTransportStatus, "label" | "location">): string {
  return transport.label?.trim() || transport.location;
}

// ================================ S3 form model ==============================

export type S3Preset = { id: string; label: string; endpoint: string; region: string; pathStyle: boolean; hint?: string };

/** Field fillers only: the native side never treats providers differently. */
export const S3_PRESETS: readonly S3Preset[] = [
  {
    id: "aws",
    label: "Amazon S3",
    endpoint: "https://s3.us-east-1.amazonaws.com",
    region: "us-east-1",
    pathStyle: false,
    hint: "If your bucket is in another region, change it in both the endpoint and the Region field.",
  },
  {
    id: "r2",
    label: "Cloudflare R2",
    endpoint: "https://ACCOUNT_ID.r2.cloudflarestorage.com",
    region: "auto",
    pathStyle: false,
    hint: "Replace ACCOUNT_ID with your Cloudflare account ID.",
  },
  {
    id: "b2",
    label: "Backblaze B2",
    endpoint: "https://s3.us-west-004.backblazeb2.com",
    region: "us-west-004",
    pathStyle: false,
    hint: "Use the endpoint and region shown on your bucket’s page.",
  },
  { id: "wasabi", label: "Wasabi", endpoint: "https://s3.us-east-1.wasabisys.com", region: "us-east-1", pathStyle: false },
  { id: "minio", label: "MinIO on this computer", endpoint: "http://127.0.0.1:9000", region: "us-east-1", pathStyle: true },
];

export type S3FormState = {
  preset: string;
  endpoint: string;
  region: string;
  bucket: string;
  prefix: string;
  pathStyle: boolean;
  label: string;
  accessKeyId: string;
  secretAccessKey: string;
  sessionToken: string;
};

export const EMPTY_S3_FORM: S3FormState = {
  preset: "",
  endpoint: "",
  region: "",
  bucket: "",
  prefix: "",
  pathStyle: false,
  label: "",
  accessKeyId: "",
  secretAccessKey: "",
  sessionToken: "",
};

export function applyS3Preset(form: S3FormState, presetId: string): S3FormState {
  const preset = S3_PRESETS.find((candidate) => candidate.id === presetId);
  if (!preset) return { ...form, preset: "" };
  return { ...form, preset: preset.id, endpoint: preset.endpoint, region: preset.region, pathStyle: preset.pathStyle };
}

/** Whether every required field has something in it; the native side does
 * the real validation. */
export function s3FormComplete(form: S3FormState): boolean {
  return [form.endpoint, form.region, form.bucket, form.accessKeyId, form.secretAccessKey].every((value) => value.trim() !== "");
}

export function s3ConfigFromForm(form: S3FormState): S3ConnectorConfig {
  return {
    endpoint: form.endpoint.trim(),
    region: form.region.trim(),
    bucket: form.bucket.trim(),
    prefix: form.prefix.trim().replace(/^\/+|\/+$/g, ""),
    pathStyle: form.pathStyle,
    label: form.label.trim() || null,
  };
}

export type S3CredentialDraft = { accessKeyId: string; secretAccessKey: string; sessionToken: string };
export const EMPTY_S3_CREDENTIALS: S3CredentialDraft = { accessKeyId: "", secretAccessKey: "", sessionToken: "" };

export function s3CredentialsComplete(draft: S3CredentialDraft): boolean {
  return draft.accessKeyId.trim() !== "" && draft.secretAccessKey.trim() !== "";
}

export function s3CredentialsFromDraft(draft: S3CredentialDraft): S3Credentials {
  return {
    accessKeyId: draft.accessKeyId.trim(),
    secretAccessKey: draft.secretAccessKey.trim(),
    sessionToken: draft.sessionToken.trim() || null,
  };
}

/** The least an access key needs: list the prefix, and read, write, and
 * delete objects under it. */
export function s3PermissionsPolicy(bucket: string, prefix: string): string {
  const name = bucket.trim() || "YOUR-BUCKET";
  const folder = prefix.trim().replace(/^\/+|\/+$/g, "");
  const objects = folder ? `${folder}/*` : "*";
  return JSON.stringify(
    {
      Version: "2012-10-17",
      Statement: [
        {
          Effect: "Allow",
          Action: ["s3:ListBucket"],
          Resource: `arn:aws:s3:::${name}`,
          Condition: { StringLike: { "s3:prefix": [objects] } },
        },
        {
          Effect: "Allow",
          Action: ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
          Resource: `arn:aws:s3:::${name}/${objects}`,
        },
      ],
    },
    null,
    2,
  );
}

export type ProbeLine = { label: string; ok: boolean };

export function s3TestChecklist(test: S3ConnectionTest): ProbeLine[] {
  return [
    { label: "Reachable", ok: test.reachable },
    { label: "Can list files", ok: test.canList },
    { label: "Can write", ok: test.canWrite },
    { label: "Can read", ok: test.canRead },
    { label: "Can delete", ok: test.canDelete },
  ];
}

export function s3TestPassed(test: S3ConnectionTest): boolean {
  return s3TestChecklist(test).every((line) => line.ok);
}

export function s3SpaceLine(test: S3ConnectionTest): string | null {
  switch (test.spacePresence) {
    case "existing": return "Existing sync group found in this bucket and folder.";
    case "none": return "Empty — ready for a new sync group.";
    case "unknown": return "Couldn’t check for an existing sync group.";
    case "legacy": return "Holds a sync group from an earlier test version, which this version can’t use.";
    default: return null;
  }
}

// ================================= Components ================================

/** Access key fields, shared by the S3 form, joining with a code that left
 * credentials out, and replacing credentials. */
export function S3CredentialFields({
  value,
  onChange,
  disabled,
}: {
  value: S3CredentialDraft;
  onChange(next: S3CredentialDraft): void;
  disabled?: boolean;
}) {
  return (
    <>
      <label className="settings-field">
        <span>Access key ID</span>
        <input type="text" autoComplete="off" spellCheck={false} value={value.accessKeyId} disabled={disabled} onChange={(event) => onChange({ ...value, accessKeyId: event.target.value })} />
      </label>
      <label className="settings-field">
        <span>Secret access key</span>
        <input type="password" autoComplete="off" value={value.secretAccessKey} disabled={disabled} onChange={(event) => onChange({ ...value, secretAccessKey: event.target.value })} />
      </label>
      <Disclosure summary="Temporary credentials">
        <label className="settings-field">
          <span>Session token (optional)</span>
          <input type="password" autoComplete="off" value={value.sessionToken} disabled={disabled} onChange={(event) => onChange({ ...value, sessionToken: event.target.value })} />
        </label>
      </Disclosure>
    </>
  );
}

function S3TestResults({ test }: { test: S3ConnectionTest }) {
  const space = s3SpaceLine(test);
  return (
    <div className="sync-probe-results">
      <ul className="sync-probe-checklist" aria-label="Connection test results">
        {s3TestChecklist(test).map((line) => (
          <li key={line.label} className={line.ok ? "sync-probe-ok" : "sync-probe-failed"}>
            <span aria-hidden="true">{line.ok ? "✓" : "✗"}</span> {line.label}
            <span className="sr-only">{line.ok ? " passed" : " failed"}</span>
          </li>
        ))}
      </ul>
      {test.error ? <p className="settings-hint sync-probe-error">{test.error}</p> : null}
      {space ? <p className="settings-hint">{space}</p> : null}
      {test.versioningEnabled ? (
        <p className="settings-hint sync-attention-text">
          This bucket keeps old versions of files. Deleted and replaced files keep using storage until the bucket’s
          lifecycle rules remove them.
        </p>
      ) : null}
    </div>
  );
}

function FolderConnectorForm({ operation, refresh, onAdded }: { operation: Operation; refresh(): Promise<void>; onAdded(): void }) {
  const { busy, setError, runFor } = operation;
  const choose = () => runFor("add-folder", async () => {
    const status = await replicatedSyncAddFolder();
    if (!status) {
      setError("No folder selected.", "add-folder");
      return;
    }
    await refresh();
    onAdded();
  });
  return (
    <>
      <p className="settings-hint">
        Choose a folder your devices already keep in sync, such as one in Dropbox, iCloud Drive, OneDrive, or Syncthing.
        Every device in the group chooses its own copy of that same folder.
      </p>
      <button type="button" className="primary-action" disabled={busy} onClick={choose}>Choose folder…</button>
      <InlineStatus operation={operation} for="add-folder" />
    </>
  );
}

function S3ConnectorForm({ operation, refresh, onAdded }: { operation: Operation; refresh(): Promise<void>; onAdded(): void }) {
  const { busy, runFor } = operation;
  const [form, setForm] = useState<S3FormState>(EMPTY_S3_FORM);
  const [test, setTest] = useState<S3ConnectionTest | null>(null);
  const update = (patch: Partial<S3FormState>) => {
    setForm((current) => ({ ...current, ...patch }));
    setTest(null);
  };
  const credentials: S3CredentialDraft = { accessKeyId: form.accessKeyId, secretAccessKey: form.secretAccessKey, sessionToken: form.sessionToken };
  const preset = S3_PRESETS.find((candidate) => candidate.id === form.preset);
  const policy = s3PermissionsPolicy(form.bucket, form.prefix);

  const runTest = () => {
    setTest(null);
    runFor("s3", async () => setTest(await replicatedSyncProbeS3(s3ConfigFromForm(form), s3CredentialsFromDraft(credentials))));
  };
  const add = () => runFor("s3", async () => {
    await replicatedSyncAddS3(s3ConfigFromForm(form), s3CredentialsFromDraft(credentials));
    setForm(EMPTY_S3_FORM);
    setTest(null);
    await refresh();
    onAdded();
  });

  return (
    <>
      <p className="settings-hint">
        Every device in the group needs this same bucket and folder. The keys are stored only in this device’s keychain
        and are never included in a settings export.
      </p>
      <label className="settings-field">
        <span>Provider</span>
        <select value={form.preset} disabled={busy} onChange={(event) => { setForm((current) => applyS3Preset(current, event.target.value)); setTest(null); }}>
          <option value="">Other S3-compatible storage</option>
          {S3_PRESETS.map((candidate) => <option key={candidate.id} value={candidate.id}>{candidate.label}</option>)}
        </select>
      </label>
      {preset?.hint ? <p className="settings-hint">{preset.hint}</p> : null}
      <label className="settings-field">
        <span>Endpoint URL</span>
        <input type="text" spellCheck={false} placeholder="https://s3.example.com" value={form.endpoint} disabled={busy} onChange={(event) => update({ endpoint: event.target.value })} />
      </label>
      <label className="settings-field">
        <span>Region</span>
        <input type="text" spellCheck={false} value={form.region} disabled={busy} onChange={(event) => update({ region: event.target.value })} />
      </label>
      <label className="settings-field">
        <span>Bucket</span>
        <input type="text" spellCheck={false} value={form.bucket} disabled={busy} onChange={(event) => update({ bucket: event.target.value })} />
      </label>
      <label className="settings-field">
        <span>Folder in the bucket (optional)</span>
        <input type="text" spellCheck={false} placeholder="threestrands" value={form.prefix} disabled={busy} onChange={(event) => update({ prefix: event.target.value })} />
      </label>
      <S3CredentialFields value={credentials} disabled={busy} onChange={(next) => update(next)} />
      <label className="settings-field">
        <span>Name (optional)</span>
        <input type="text" maxLength={MAX_CONNECTOR_LABEL_CHARS} value={form.label} disabled={busy} onChange={(event) => update({ label: event.target.value })} />
      </label>
      <Disclosure summary="Advanced">
        <label className="settings-checkbox">
          <input type="checkbox" checked={form.pathStyle} disabled={busy} onChange={(event) => update({ pathStyle: event.target.checked })} />
          <span>Use path-style addressing (needed for IP-address and most self-hosted endpoints)</span>
        </label>
      </Disclosure>
      <Disclosure summary="Permissions this key needs">
        <p className="settings-hint">A key limited to this bucket and folder is safest. Reading the bucket’s versioning setting is optional.</p>
        <pre className="sync-policy" aria-label="Minimal access policy">{policy}</pre>
        <button type="button" className="account-action-button" onClick={() => void navigator.clipboard?.writeText(policy)}>Copy policy</button>
      </Disclosure>
      <div className="settings-row">
        <button type="button" className="account-action-button" disabled={busy || !s3FormComplete(form)} onClick={runTest}>Test connection</button>
        <button type="button" className="primary-action" disabled={busy || !test || !s3TestPassed(test)} onClick={add}>Add connector</button>
      </div>
      {test ? <S3TestResults test={test} /> : <p className="settings-hint">Test the connection before adding it.</p>}
      <InlineStatus operation={operation} for="s3" />
    </>
  );
}

function IpfsConnectorForm({ operation, refresh, onAdded }: { operation: Operation; refresh(): Promise<void>; onAdded(): void }) {
  const { busy, setError, runFor } = operation;
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [probe, setProbe] = useState<IpfsRpcProbeReport | null>(null);

  const test = () => {
    setProbe(null);
    runFor("ipfs", async () => {
      const report = await replicatedSyncProbeIpfsRpc(baseUrl, token.trim() ? token : null);
      setProbe(report);
      if (!report.versionOk) setError("Could not reach an IPFS RPC endpoint at that URL.", "ipfs");
    });
  };
  const add = () => runFor("ipfs", async () => {
    const status = await replicatedSyncAddIpfsRpc(baseUrl, token.trim() ? token : null);
    if (!status) {
      setError("Could not add that endpoint.", "ipfs");
      return;
    }
    setBaseUrl("");
    setToken("");
    setProbe(null);
    await refresh();
    onAdded();
  });

  return (
    <>
      <p className="settings-hint">
        Point at a Kubo-compatible RPC endpoint or a dedicated Filebase bucket. For Filebase, create one bucket for this
        sync group, generate its bucket-specific RPC token, and enter that same token on every device in the group. The
        token is stored only in this device&apos;s OS keychain, never in a settings export.
      </p>
      <label className="settings-field">
        <span>RPC base URL</span>
        <input type="text" placeholder="https://ipfs.example.com:5001" value={baseUrl} disabled={busy} onChange={(event) => { setBaseUrl(event.target.value); setProbe(null); }} />
      </label>
      <label className="settings-field">
        <span>Access token (optional)</span>
        <input type="password" value={token} disabled={busy} onChange={(event) => { setToken(event.target.value); setProbe(null); }} />
      </label>
      <div className="settings-row">
        <button type="button" className="account-action-button" disabled={busy} onClick={() => { setBaseUrl(FILEBASE_RPC_URL); setProbe(null); }}>
          Fill in Filebase URL
        </button>
        <button type="button" className="account-action-button" disabled={busy || !baseUrl} onClick={test}>Test connection</button>
      </div>
      {probe?.versionOk ? (
        <p className="settings-hint">{`Reachable · ${probe.headDiscoveryAvailable ? "supports sync discovery through bucket pins" : "bucket pins unavailable"}`}</p>
      ) : null}
      <button type="button" className="primary-action" disabled={busy || !baseUrl} onClick={add}>Add IPFS RPC endpoint</button>
      <InlineStatus operation={operation} for="ipfs" />
    </>
  );
}

const CONNECTOR_CHOICES: readonly { kind: ConnectorKind; pitch: string; tradeoff: string }[] = [
  { kind: "folder", pitch: "A folder your devices already sync (Dropbox, iCloud Drive, Syncthing…)", tradeoff: "Needs a folder-sync app on every device" },
  { kind: "s3", pitch: "A bucket you own on AWS, Cloudflare R2, Backblaze B2, MinIO…", tradeoff: "Needs an access key that can read and write one bucket" },
  { kind: "ipfs_rpc", pitch: "A Kubo-compatible RPC endpoint or a dedicated Filebase bucket", tradeoff: "Advanced" },
];

/** The three connector kinds, then the chosen kind's form. */
export function ConnectorPicker({ operation, refresh, onDone }: { operation: Operation; refresh(): Promise<void>; onDone?(): void }) {
  const [kind, setKind] = useState<ConnectorKind | null>(null);
  const finish = () => {
    setKind(null);
    onDone?.();
  };
  if (kind === null) {
    return (
      <div className="sync-connector-choices" role="group" aria-label="Connector type">
        {CONNECTOR_CHOICES.map((choice) => (
          <button key={choice.kind} type="button" className="sync-connector-choice" disabled={operation.busy} onClick={() => setKind(choice.kind)}>
            <strong>{connectorKindLabel(choice.kind)}</strong>
            <span>{choice.pitch}</span>
            <span className="settings-hint">{choice.tradeoff}</span>
          </button>
        ))}
      </div>
    );
  }
  const forms: Record<ConnectorKind, ReactNode> = {
    folder: <FolderConnectorForm operation={operation} refresh={refresh} onAdded={finish} />,
    s3: <S3ConnectorForm operation={operation} refresh={refresh} onAdded={finish} />,
    ipfs_rpc: <IpfsConnectorForm operation={operation} refresh={refresh} onAdded={finish} />,
  };
  return (
    <div className="sync-connector-form" role="group" aria-label={`Add ${connectorKindLabel(kind)}`}>
      <div className="settings-row">
        <button type="button" className="account-action-button" disabled={operation.busy} onClick={() => setKind(null)}>← Back</button>
        <h4>{connectorKindLabel(kind)}</h4>
      </div>
      {forms[kind]}
    </div>
  );
}

function disconnectExplanation(transport: ReplicatedSyncTransportStatus): string {
  switch (transport.kind) {
    case "s3":
      return `“Delete files” removes this group’s encrypted files from ${transport.location}. If the bucket keeps versions, old versions stay until its lifecycle rules remove them.`;
    case "ipfs_rpc":
      return "Pinned objects remain with the provider until you remove them there.";
    default:
      return "You can keep the encrypted files for another device or delete this device’s copy.";
  }
}

function ReplaceCredentials({
  transport,
  operation,
  refresh,
  onDone,
}: {
  transport: ReplicatedSyncTransportStatus;
  operation: Operation;
  refresh(): Promise<void>;
  onDone(): void;
}) {
  const { busy, runFor } = operation;
  const key = `transport:${transport.instanceId}`;
  const [s3, setS3] = useState<S3CredentialDraft>(EMPTY_S3_CREDENTIALS);
  const [token, setToken] = useState("");
  const ready = transport.kind === "s3" ? s3CredentialsComplete(s3) : token.trim() !== "";

  const save = () => runFor(key, async () => {
    if (transport.kind === "s3") {
      if (!transport.s3Config) throw new Error("This connector’s settings aren’t available. Disconnect it and add it again.");
      const credentials = s3CredentialsFromDraft(s3);
      const test = await replicatedSyncProbeS3(transport.s3Config, credentials);
      if (!s3TestPassed(test)) throw new Error(test.error ?? "Those credentials didn’t pass the connection test.");
      await replicatedSyncUpdateConnector(transport.instanceId, { credentials: { kind: "s3", ...credentials } });
    } else {
      const report = await replicatedSyncProbeIpfsRpc(transport.location, token);
      if (!report.versionOk) throw new Error("That token didn’t pass the connection test.");
      await replicatedSyncUpdateConnector(transport.instanceId, { credentials: { kind: "ipfs_rpc", token } });
    }
    setS3(EMPTY_S3_CREDENTIALS);
    setToken("");
    await refresh();
    onDone();
  });

  return (
    <div className="settings-inline-panel" role="group" aria-label="Replace credentials">
      <p>New credentials are tested against this connector before they replace the old ones.</p>
      {transport.kind === "s3" ? (
        <S3CredentialFields value={s3} disabled={busy} onChange={setS3} />
      ) : (
        <label className="settings-field">
          <span>New access token</span>
          <input type="password" autoComplete="off" value={token} disabled={busy} onChange={(event) => setToken(event.target.value)} />
        </label>
      )}
      <span className="settings-inline-confirm-actions">
        <button type="button" disabled={busy} onClick={onDone}>Cancel</button>
        <button type="button" className="primary-action" disabled={busy || !ready} onClick={save}>Test and save</button>
      </span>
    </div>
  );
}

export function ConnectorCard({ transport, operation, refresh }: { transport: ReplicatedSyncTransportStatus; operation: Operation; refresh(): Promise<void> }) {
  const { busy, actFor } = operation;
  const [panel, setPanel] = useState<"none" | "edit" | "credentials" | "disconnect">("none");
  const [name, setName] = useState(transport.label ?? "");
  const nameId = useId();
  const health = describeTransportHealth(transport);
  const key = `transport:${transport.instanceId}`;
  const hasCredentials = transport.kind === "s3" || transport.kind === "ipfs_rpc";

  return (
    <li className={`account-card sync-location-${health.tone}`}>
      <div className="account-card-row">
        <div className="account-card-identity">
          <strong>{connectorDisplayName(transport)}</strong>
          <span className="account-card-email">
            <span className="sync-kind-badge">{connectorKindLabel(transport.kind)}</span> · <span className="sync-location-health">{health.label}</span>
            {transport.storageBytes != null ? ` · ${formatStorageEstimate(transport.storageBytes)}` : ""}
          </span>
          {transport.label?.trim() ? <span className="account-card-email">{transport.location}</span> : null}
          {health.detail ? <span className="account-card-email">{health.detail}</span> : null}
          {!transport.headDiscovery ? (
            <span className="account-card-email">Storage only: other devices can’t discover new changes through this connector on its own.</span>
          ) : null}
          {transport.lastSuccessAt ? <span className="account-card-email">Last synced {new Date(transport.lastSuccessAt).toLocaleString()}</span> : null}
        </div>
        <button type="button" className="account-action-button" disabled={busy} aria-expanded={panel === "edit"} onClick={() => { setName(transport.label ?? ""); setPanel("edit"); }}>
          Edit…
        </button>
        <button type="button" className="account-action-button danger-action" disabled={busy} aria-expanded={panel === "disconnect"} onClick={() => setPanel("disconnect")}>
          Disconnect…
        </button>
      </div>
      {panel === "edit" ? (
        <form
          className="settings-inline-panel"
          aria-label="Edit connector"
          onSubmit={(event) => {
            event.preventDefault();
            setPanel("none");
            actFor(key, () => replicatedSyncUpdateConnector(transport.instanceId, { label: name }));
          }}
        >
          <label className="settings-field" htmlFor={nameId}>
            <span>Name</span>
            <input id={nameId} type="text" maxLength={MAX_CONNECTOR_LABEL_CHARS} placeholder={transport.location} value={name} disabled={busy} autoFocus onChange={(event) => setName(event.target.value)} />
          </label>
          <span className="settings-inline-confirm-actions">
            {hasCredentials ? <button type="button" disabled={busy} onClick={() => setPanel("credentials")}>Replace credentials…</button> : null}
            <button type="button" disabled={busy} onClick={() => setPanel("none")}>Cancel</button>
            <button type="submit" className="primary-action" disabled={busy}>Save</button>
          </span>
        </form>
      ) : null}
      {panel === "credentials" ? <ReplaceCredentials transport={transport} operation={operation} refresh={refresh} onDone={() => setPanel("none")} /> : null}
      {panel === "disconnect" ? (
        <InlineConfirm
          ariaLabel="Disconnect connector confirmation"
          cancelLabel="Cancel"
          onCancel={() => setPanel("none")}
          disabled={busy}
          actions={[
            { label: "Disconnect and keep data", onClick: () => { setPanel("none"); actFor(key, () => replicatedSyncRemoveTransport(transport.instanceId, false)); } },
            ...(transport.supportsDeleteData ? [{ label: "Delete files and disconnect", className: "danger-action", onClick: () => { setPanel("none"); actFor(key, () => replicatedSyncRemoveTransport(transport.instanceId, true)); } }] : []),
          ]}
        >
          <strong>Stop syncing through this connector?</strong><br />{disconnectExplanation(transport)}
        </InlineConfirm>
      ) : null}
      <InlineStatus operation={operation} for={key} />
    </li>
  );
}

/** The configured connectors plus the picker to add another. Rendered as
 * setup step 1 and under "Connectors" once enrolled. */
export function ConnectorList({
  transports,
  operation,
  refresh,
}: {
  transports: ReplicatedSyncTransportStatus[];
  operation: Operation;
  refresh(): Promise<void>;
}) {
  const [adding, setAdding] = useState(false);
  return (
    <>
      <p className="settings-hint">
        Every device in a sync group uses the same connector: the same shared folder, bucket, or endpoint. Don’t reuse
        a bucket or folder for a separate sync group.
      </p>
      {transports.length > 0 ? (
        <ul className="accounts-list" aria-label="Connectors">
          {transports.map((transport) => <ConnectorCard key={transport.instanceId} transport={transport} operation={operation} refresh={refresh} />)}
        </ul>
      ) : null}
      {transports.length === 0 ? (
        <ConnectorPicker operation={operation} refresh={refresh} />
      ) : adding ? (
        <>
          <p className="settings-hint">
            Adding a second connector keeps your devices syncing if one provider is down. Existing changes are copied to
            it automatically.
          </p>
          <ConnectorPicker operation={operation} refresh={refresh} onDone={() => setAdding(false)} />
          <button type="button" className="account-action-button" disabled={operation.busy} onClick={() => setAdding(false)}>Cancel</button>
        </>
      ) : (
        <button type="button" className="account-action-button" disabled={operation.busy} onClick={() => setAdding(true)}>Add another connector</button>
      )}
    </>
  );
}
