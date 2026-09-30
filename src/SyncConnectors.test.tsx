import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./replicatedSync");

import * as sync from "./replicatedSync";
import { useSettingsOperation } from "./settingsOperations";
import {
  applyS3Preset,
  ConnectorCard,
  connectorDisplayName,
  connectorKindLabel,
  ConnectorList,
  EMPTY_S3_FORM,
  s3ConfigFromForm,
  s3FormComplete,
  s3PermissionsPolicy,
  s3SpaceLine,
  s3TestChecklist,
  s3TestPassed,
} from "./SyncConnectors";
import type { Operation } from "./syncSettingsParts";

const refresh = vi.fn(async () => {});

function Harness({ children }: { children: (operation: Operation) => ReactNode }) {
  const operation = useSettingsOperation(refresh);
  return <>{children(operation)}</>;
}

const passing: sync.S3ConnectionTest = {
  reachable: true,
  canList: true,
  canWrite: true,
  canRead: true,
  canDelete: true,
  versioningEnabled: false,
  error: null,
  spacePresence: "none",
};

const s3Status: sync.ReplicatedSyncTransportStatus = {
  instanceId: "s3-1",
  kind: "s3",
  label: null,
  location: "https://s3.example.com · sync-bucket/team",
  supportsDeleteData: true,
  s3Config: { endpoint: "https://s3.example.com", region: "auto", bucket: "sync-bucket", prefix: "team", pathStyle: false, label: null },
  health: "healthy",
  headDiscovery: true,
  pending: 0,
  delivered: 0,
  failed: 0,
};

beforeEach(() => {
  vi.clearAllMocks();
  Object.defineProperty(navigator, "clipboard", { value: { writeText: vi.fn(async () => {}) }, configurable: true });
});
afterEach(cleanup);

describe("connector naming", () => {
  it("labels each kind and prefers a connector's name over its location", () => {
    expect(["folder", "s3", "ipfs_rpc"].map(connectorKindLabel)).toEqual(["Shared folder", "S3 storage", "IPFS"]);
    expect(connectorDisplayName({ label: "Team bucket", location: "x" })).toBe("Team bucket");
    expect(connectorDisplayName({ label: "  ", location: "x" })).toBe("x");
    expect(connectorDisplayName({ label: null, location: "x" })).toBe("x");
  });
});

describe("the S3 form model", () => {
  it("fills endpoint, region, and path style from a preset without touching the rest", () => {
    const form = applyS3Preset({ ...EMPTY_S3_FORM, bucket: "b-1", accessKeyId: "AKIA" }, "minio");
    expect(form).toMatchObject({ preset: "minio", endpoint: "http://127.0.0.1:9000", region: "us-east-1", pathStyle: true, bucket: "b-1", accessKeyId: "AKIA" });
    expect(applyS3Preset(form, "r2")).toMatchObject({ region: "auto", pathStyle: false });
    expect(applyS3Preset(form, "").preset).toBe("");
  });

  it("needs endpoint, region, bucket, and both key parts before testing", () => {
    const complete = { ...EMPTY_S3_FORM, endpoint: "https://s3.example.com", region: "auto", bucket: "b", accessKeyId: "a", secretAccessKey: "s" };
    expect(s3FormComplete(complete)).toBe(true);
    for (const field of ["endpoint", "region", "bucket", "accessKeyId", "secretAccessKey"] as const) {
      expect(s3FormComplete({ ...complete, [field]: "  " })).toBe(false);
    }
    expect(s3ConfigFromForm({ ...complete, prefix: " /team/sync/ ", label: " " })).toEqual({
      endpoint: "https://s3.example.com",
      region: "auto",
      bucket: "b",
      prefix: "team/sync",
      pathStyle: false,
      label: null,
    });
  });

  it("builds a minimal policy scoped to the bucket and folder", () => {
    const scoped = JSON.parse(s3PermissionsPolicy("sync-bucket", "/team/"));
    expect(scoped.Statement[0]).toEqual({
      Effect: "Allow",
      Action: ["s3:ListBucket"],
      Resource: "arn:aws:s3:::sync-bucket",
      Condition: { StringLike: { "s3:prefix": ["team/*"] } },
    });
    expect(scoped.Statement[1].Resource).toBe("arn:aws:s3:::sync-bucket/team/*");
    expect(scoped.Statement[1].Action).toEqual(["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]);
    expect(JSON.parse(s3PermissionsPolicy("", "")).Statement[1].Resource).toBe("arn:aws:s3:::YOUR-BUCKET/*");
  });

  it("summarizes a connection test", () => {
    expect(s3TestPassed(passing)).toBe(true);
    expect(s3TestPassed({ ...passing, canDelete: false })).toBe(false);
    expect(s3TestChecklist({ ...passing, canWrite: false }).find((line) => line.label === "Can write")?.ok).toBe(false);
    expect(s3SpaceLine({ ...passing, spacePresence: "existing" })).toMatch(/Existing sync group/);
    expect(s3SpaceLine({ ...passing, spacePresence: "none" })).toMatch(/ready for a new sync group/);
    expect(s3SpaceLine({ ...passing, spacePresence: "legacy" })).toMatch(/earlier test version/);
    expect(s3SpaceLine({ ...passing, spacePresence: null })).toBeNull();
  });
});

async function openS3Form() {
  render(<Harness>{(operation) => <ConnectorList transports={[]} operation={operation} refresh={refresh} />}</Harness>);
  fireEvent.click(screen.getByRole("button", { name: /^S3 storage/ }));
  return screen.getByRole("group", { name: "Add S3 storage" });
}

function fillS3(form: HTMLElement) {
  const set = (name: string, value: string) => fireEvent.change(within(form).getByLabelText(name), { target: { value } });
  set("Endpoint URL", "https://s3.example.com");
  set("Region", "auto");
  set("Bucket", "sync-bucket");
  set("Folder in the bucket (optional)", "team");
  set("Access key ID", "AKIAEXAMPLE");
  set("Secret access key", "s3-secret");
}

describe("adding an S3 connector", () => {
  it("keeps the secret in a password field and requires a passing test before adding", async () => {
    vi.mocked(sync.replicatedSyncProbeS3).mockResolvedValue(passing);
    vi.mocked(sync.replicatedSyncAddS3).mockResolvedValue(null);
    const form = await openS3Form();
    expect(within(form).getByLabelText("Secret access key")).toHaveAttribute("type", "password");
    const test = within(form).getByRole("button", { name: "Test connection" });
    const add = within(form).getByRole("button", { name: "Add connector" });
    expect(test).toBeDisabled();
    expect(add).toBeDisabled();

    fillS3(form);
    expect(add).toBeDisabled();
    fireEvent.click(test);
    await waitFor(() => expect(add).toBeEnabled());
    expect(sync.replicatedSyncProbeS3).toHaveBeenCalledWith(
      { endpoint: "https://s3.example.com", region: "auto", bucket: "sync-bucket", prefix: "team", pathStyle: false, label: null },
      { accessKeyId: "AKIAEXAMPLE", secretAccessKey: "s3-secret", sessionToken: null },
    );
    expect(within(form).getByText("Empty — ready for a new sync group.")).toBeInTheDocument();

    // Editing any field throws the result away.
    fireEvent.change(within(form).getByLabelText("Bucket"), { target: { value: "other-bucket" } });
    expect(add).toBeDisabled();
    expect(within(form).queryByRole("list", { name: "Connection test results" })).not.toBeInTheDocument();

    fireEvent.click(test);
    await waitFor(() => expect(add).toBeEnabled());
    fireEvent.click(add);
    await waitFor(() => expect(sync.replicatedSyncAddS3).toHaveBeenCalledTimes(1));
    expect(vi.mocked(sync.replicatedSyncAddS3).mock.calls[0]![0].bucket).toBe("other-bucket");
    await waitFor(() => expect(refresh).toHaveBeenCalled());
  });

  it("explains a failed check and a versioned bucket, and keeps Add disabled", async () => {
    vi.mocked(sync.replicatedSyncProbeS3).mockResolvedValue({
      ...passing,
      canDelete: false,
      versioningEnabled: true,
      error: "transport authentication failed: AccessDenied: denied",
    });
    const form = await openS3Form();
    fillS3(form);
    fireEvent.click(within(form).getByRole("button", { name: "Test connection" }));

    const results = await within(form).findByRole("list", { name: "Connection test results" });
    expect(within(results).getByText(/Can delete/).closest("li")).toHaveClass("sync-probe-failed");
    expect(within(results).getByText(/Can write/).closest("li")).toHaveClass("sync-probe-ok");
    expect(within(form).getByText(/AccessDenied/)).toBeInTheDocument();
    expect(within(form).getByText(/keeps old versions of files/)).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: "Add connector" })).toBeDisabled();
  });

  it("offers presets and a copyable permissions policy", async () => {
    const form = await openS3Form();
    fireEvent.change(within(form).getByLabelText("Provider"), { target: { value: "r2" } });
    expect(within(form).getByLabelText("Region")).toHaveValue("auto");
    expect(within(form).getByText(/Replace ACCOUNT_ID/)).toBeInTheDocument();
    fireEvent.change(within(form).getByLabelText("Bucket"), { target: { value: "sync-bucket" } });
    expect(within(form).getByLabelText("Minimal access policy")).toHaveTextContent("arn:aws:s3:::sync-bucket");
    fireEvent.click(within(form).getByRole("button", { name: "Copy policy" }));
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(expect.stringContaining("s3:ListBucket"));
  });
});

async function openIpfsForm() {
  render(<Harness>{(operation) => <ConnectorList transports={[]} operation={operation} refresh={refresh} />}</Harness>);
  fireEvent.click(screen.getByRole("button", { name: /^IPFS/ }));
  return screen.getByRole("group", { name: "Add IPFS" });
}

function fillIpfs(form: HTMLElement, baseUrl: string, token: string) {
  fireEvent.change(within(form).getByLabelText("RPC base URL"), { target: { value: baseUrl } });
  fireEvent.change(within(form).getByLabelText("Access token (optional)"), { target: { value: token } });
}

const ipfsStatus: sync.ReplicatedSyncTransportStatus = {
  ...s3Status,
  instanceId: "ipfs-1",
  kind: "ipfs_rpc",
  location: "https://rpc.filebase.io",
  supportsDeleteData: false,
  s3Config: null,
};

describe("adding an IPFS connector", () => {
  it("keeps the token in a password field and probes the endpoint with it", async () => {
    vi.mocked(sync.replicatedSyncProbeIpfsRpc).mockResolvedValue({ versionOk: true, headDiscoveryAvailable: true });
    const form = await openIpfsForm();
    expect(within(form).getByLabelText("Access token (optional)")).toHaveAttribute("type", "password");
    const test = within(form).getByRole("button", { name: "Test connection" });
    expect(test).toBeDisabled();
    expect(within(form).getByRole("button", { name: "Add IPFS RPC endpoint" })).toBeDisabled();

    fillIpfs(form, "https://rpc.filebase.io", "bucket-token");
    fireEvent.click(test);

    expect(await within(form).findByText("Reachable · supports sync discovery through bucket pins")).toBeInTheDocument();
    expect(sync.replicatedSyncProbeIpfsRpc).toHaveBeenCalledWith("https://rpc.filebase.io", "bucket-token");
    expect(sync.replicatedSyncAddIpfsRpc).not.toHaveBeenCalled();

    // Editing the URL throws the result away.
    fireEvent.change(within(form).getByLabelText("RPC base URL"), { target: { value: "https://ipfs.example.com:5001" } });
    expect(within(form).queryByText(/^Reachable/)).not.toBeInTheDocument();
  });

  it("says when bucket pins are unavailable on a reachable endpoint", async () => {
    vi.mocked(sync.replicatedSyncProbeIpfsRpc).mockResolvedValue({ versionOk: true, headDiscoveryAvailable: false });
    const form = await openIpfsForm();
    fillIpfs(form, "https://ipfs.example.com:5001", "");
    fireEvent.click(within(form).getByRole("button", { name: "Test connection" }));

    expect(await within(form).findByText("Reachable · bucket pins unavailable")).toBeInTheDocument();
  });

  it("explains an endpoint that does not answer as IPFS RPC", async () => {
    vi.mocked(sync.replicatedSyncProbeIpfsRpc).mockResolvedValue({ versionOk: false, headDiscoveryAvailable: false });
    const form = await openIpfsForm();
    fillIpfs(form, "https://not-ipfs.example.com", "bucket-token");
    fireEvent.click(within(form).getByRole("button", { name: "Test connection" }));

    expect(await within(form).findByText("Could not reach an IPFS RPC endpoint at that URL.")).toBeInTheDocument();
    expect(within(form).queryByText(/^Reachable/)).not.toBeInTheDocument();
    expect(sync.replicatedSyncAddIpfsRpc).not.toHaveBeenCalled();
  });

  it("sends a blank or whitespace-only token as no token", async () => {
    vi.mocked(sync.replicatedSyncProbeIpfsRpc).mockResolvedValue({ versionOk: true, headDiscoveryAvailable: false });
    vi.mocked(sync.replicatedSyncAddIpfsRpc).mockResolvedValue(ipfsStatus);
    const form = await openIpfsForm();
    fillIpfs(form, "https://ipfs.example.com:5001", "   ");
    fireEvent.click(within(form).getByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(sync.replicatedSyncProbeIpfsRpc).toHaveBeenCalledWith("https://ipfs.example.com:5001", null));

    await waitFor(() => expect(within(form).getByRole("button", { name: "Add IPFS RPC endpoint" })).toBeEnabled());
    fireEvent.click(within(form).getByRole("button", { name: "Add IPFS RPC endpoint" }));
    await waitFor(() => expect(sync.replicatedSyncAddIpfsRpc).toHaveBeenCalledWith("https://ipfs.example.com:5001", null));
  });

  it("adds the endpoint with its token, refreshes, and returns to the connector choices", async () => {
    vi.mocked(sync.replicatedSyncAddIpfsRpc).mockResolvedValue(ipfsStatus);
    const form = await openIpfsForm();
    fillIpfs(form, "https://rpc.filebase.io", "bucket-token");
    fireEvent.click(within(form).getByRole("button", { name: "Add IPFS RPC endpoint" }));

    await waitFor(() => expect(sync.replicatedSyncAddIpfsRpc).toHaveBeenCalledWith("https://rpc.filebase.io", "bucket-token"));
    expect(sync.replicatedSyncAddIpfsRpc).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(refresh).toHaveBeenCalled());
    expect(await screen.findByRole("group", { name: "Connector type" })).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Add IPFS" })).not.toBeInTheDocument();
  });

  it("keeps the form and explains when the endpoint could not be added", async () => {
    vi.mocked(sync.replicatedSyncAddIpfsRpc).mockResolvedValue(null);
    const form = await openIpfsForm();
    fillIpfs(form, "https://rpc.filebase.io", "bucket-token");
    fireEvent.click(within(form).getByRole("button", { name: "Add IPFS RPC endpoint" }));

    expect(await within(form).findByText("Could not add that endpoint.")).toBeInTheDocument();
    expect(within(form).getByLabelText("RPC base URL")).toHaveValue("https://rpc.filebase.io");
    expect(refresh).not.toHaveBeenCalled();
  });
});

describe("connector cards", () => {
  function renderCard(transport: sync.ReplicatedSyncTransportStatus) {
    render(<Harness>{(operation) => <ul><ConnectorCard transport={transport} operation={operation} refresh={refresh} /></ul>}</Harness>);
    return screen.getByRole("listitem");
  }

  it("shows the kind and falls back to the location when unnamed", () => {
    const card = renderCard(s3Status);
    expect(within(card).getByText(s3Status.location, { selector: "strong" })).toBeInTheDocument();
    expect(within(card).getByText("S3 storage")).toBeInTheDocument();
    cleanup();
    const named = renderCard({ ...s3Status, label: "Team bucket" });
    expect(within(named).getByText("Team bucket", { selector: "strong" })).toBeInTheDocument();
    expect(within(named).getByText(s3Status.location)).toBeInTheDocument();
  });

  it("renames a connector", async () => {
    vi.mocked(sync.replicatedSyncUpdateConnector).mockResolvedValue(null);
    const card = renderCard(s3Status);
    fireEvent.click(within(card).getByRole("button", { name: "Edit…" }));
    const name = within(card).getByLabelText("Name");
    expect(name).toHaveAttribute("maxLength", "60");
    fireEvent.change(name, { target: { value: "Team bucket" } });
    fireEvent.click(within(card).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sync.replicatedSyncUpdateConnector).toHaveBeenCalledWith("s3-1", { label: "Team bucket" }));
  });

  it("tests replacement S3 credentials against the same bucket before saving them", async () => {
    vi.mocked(sync.replicatedSyncProbeS3).mockResolvedValue(passing);
    vi.mocked(sync.replicatedSyncUpdateConnector).mockResolvedValue(null);
    const card = renderCard(s3Status);
    fireEvent.click(within(card).getByRole("button", { name: "Edit…" }));
    fireEvent.click(within(card).getByRole("button", { name: "Replace credentials…" }));
    const panel = within(card).getByRole("group", { name: "Replace credentials" });
    const save = within(panel).getByRole("button", { name: "Test and save" });
    expect(save).toBeDisabled();
    fireEvent.change(within(panel).getByLabelText("Access key ID"), { target: { value: "AKIANEW" } });
    fireEvent.change(within(panel).getByLabelText("Secret access key"), { target: { value: "new-secret" } });
    fireEvent.click(save);

    await waitFor(() => expect(sync.replicatedSyncUpdateConnector).toHaveBeenCalledWith("s3-1", {
      credentials: { kind: "s3", accessKeyId: "AKIANEW", secretAccessKey: "new-secret", sessionToken: null },
    }));
    expect(sync.replicatedSyncProbeS3).toHaveBeenCalledWith(s3Status.s3Config, { accessKeyId: "AKIANEW", secretAccessKey: "new-secret", sessionToken: null });
  });

  it("keeps the old credentials when the new ones fail the test", async () => {
    vi.mocked(sync.replicatedSyncProbeS3).mockResolvedValue({ ...passing, canList: false, canWrite: false, canRead: false, canDelete: false, error: "SignatureDoesNotMatch" });
    const card = renderCard(s3Status);
    fireEvent.click(within(card).getByRole("button", { name: "Edit…" }));
    fireEvent.click(within(card).getByRole("button", { name: "Replace credentials…" }));
    const panel = within(card).getByRole("group", { name: "Replace credentials" });
    fireEvent.change(within(panel).getByLabelText("Access key ID"), { target: { value: "AKIANEW" } });
    fireEvent.change(within(panel).getByLabelText("Secret access key"), { target: { value: "wrong" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Test and save" }));

    expect(await within(card).findByText("SignatureDoesNotMatch")).toBeInTheDocument();
    expect(sync.replicatedSyncUpdateConnector).not.toHaveBeenCalled();
  });

  it("offers credential replacement for IPFS but not for folders", () => {
    const ipfs = renderCard({ ...s3Status, kind: "ipfs_rpc", location: "https://rpc.filebase.io", s3Config: null, supportsDeleteData: false });
    fireEvent.click(within(ipfs).getByRole("button", { name: "Edit…" }));
    expect(within(ipfs).getByRole("button", { name: "Replace credentials…" })).toBeInTheDocument();
    cleanup();
    const folder = renderCard({ ...s3Status, kind: "folder", location: "/Users/me/Sync", s3Config: null });
    fireEvent.click(within(folder).getByRole("button", { name: "Edit…" }));
    expect(within(folder).queryByRole("button", { name: "Replace credentials…" })).not.toBeInTheDocument();
  });

  it("words the disconnect confirmation per kind", async () => {
    vi.mocked(sync.replicatedSyncRemoveTransport).mockResolvedValue(undefined);
    const s3 = renderCard(s3Status);
    fireEvent.click(within(s3).getByRole("button", { name: "Disconnect…" }));
    const confirm = within(s3).getByRole("group", { name: "Disconnect connector confirmation" });
    expect(confirm).toHaveTextContent(`removes this group’s encrypted files from ${s3Status.location}`);
    expect(confirm).toHaveTextContent(/old versions stay/);
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete files and disconnect" }));
    await waitFor(() => expect(sync.replicatedSyncRemoveTransport).toHaveBeenCalledWith("s3-1", true));
    cleanup();

    const ipfs = renderCard({ ...s3Status, kind: "ipfs_rpc", location: "https://rpc.filebase.io", supportsDeleteData: false });
    fireEvent.click(within(ipfs).getByRole("button", { name: "Disconnect…" }));
    expect(within(ipfs).getByText(/Pinned objects remain with the provider/)).toBeInTheDocument();
    expect(within(ipfs).queryByRole("button", { name: "Delete files and disconnect" })).not.toBeInTheDocument();
  });
});

describe("adding another connector", () => {
  it("explains redundancy and returns to the list afterwards", () => {
    render(<Harness>{(operation) => <ConnectorList transports={[s3Status]} operation={operation} refresh={refresh} />}</Harness>);
    expect(screen.queryByRole("group", { name: "Connector type" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add another connector" }));
    expect(screen.getByText(/keeps your devices syncing if one provider is down/)).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Connector type" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("button", { name: "Add another connector" })).toBeInTheDocument();
  });
});
