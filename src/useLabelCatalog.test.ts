import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Account, Label } from "./domain";
import { mailClient } from "./data/client";
import { conversationLabelGroups } from "./labels";
import { useLabelCatalog } from "./useLabelCatalog";
import type { MailSyncActivityEvent } from "./useMailSyncActivity";

const listenMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

const account = "me@example.com";
const otherAccount = "other@example.com";
const accounts: Account[] = [];
const inbox: Label = { id: "INBOX", name: "Inbox", kind: "system" };
const clients: Label = { id: "lf:Clients", name: "Clients", kind: "user" };
const projects: Label = { id: "folder:Folders/Projects", name: "Folders/Projects", kind: "folder" };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("useLabelCatalog", () => {
  let emit: (payload: MailSyncActivityEvent) => void;
  const unlisten = vi.fn();

  beforeEach(() => {
    emit = () => {};
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    listenMock.mockImplementation(async (_event: string, handler: (event: { payload: MailSyncActivityEvent }) => void) => {
      emit = (payload) => handler({ payload });
      return unlisten;
    });
    vi.spyOn(mailClient, "mailSyncActivity").mockResolvedValue([]);
    vi.spyOn(mailClient, "listLabels").mockResolvedValue([inbox]);
  });

  afterEach(() => {
    cleanup();
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    vi.restoreAllMocks();
    listenMock.mockReset();
    unlisten.mockReset();
  });

  function renderCatalog(target: string | undefined = undefined) {
    return renderHook(({ targetAccount }) => useLabelCatalog(accounts, account, targetAccount, async () => {}), {
      initialProps: { targetAccount: target },
    });
  }

  it.each([clients, projects])("resolves newly synced $kind labels after the catalog was cached", async (added) => {
    const { result } = renderCatalog();
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox]));
    expect(conversationLabelGroups([inbox.id, added.id], result.current.labelsByAccount[account]))
      .toEqual({ systemLabelNames: ["Inbox"], userLabels: [] });

    vi.mocked(mailClient.listLabels).mockResolvedValue([inbox, added]);
    act(() => emit({ accountId: account, active: true }));
    expect(mailClient.listLabels).toHaveBeenCalledTimes(1);
    // A sync finish is enough, even without an unread-counts-changed event.
    act(() => emit({ accountId: account, active: false }));
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox, added]));

    expect(conversationLabelGroups([inbox.id, added.id], result.current.labelsByAccount[account])).toEqual({
      systemLabelNames: added.kind === "folder" ? ["Inbox", added.name] : ["Inbox"],
      userLabels: added.kind === "user" ? [added] : [],
    });
    expect(mailClient.listLabels).toHaveBeenCalledTimes(2);
  });

  it("refreshes a cached account when Manage Labels opens, including after closing and reopening", async () => {
    const { result, rerender } = renderCatalog();
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox]));

    vi.mocked(mailClient.listLabels).mockResolvedValue([inbox, clients]);
    rerender({ targetAccount: account });
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox, clients]));

    rerender({ targetAccount: undefined });
    vi.mocked(mailClient.listLabels).mockResolvedValue([inbox, clients, projects]);
    rerender({ targetAccount: account });
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox, clients, projects]));
    expect(mailClient.listLabels).toHaveBeenCalledTimes(3);
  });

  it("keeps account catalogs separate when a background account finishes syncing", async () => {
    const { result } = renderCatalog();
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox]));
    vi.mocked(mailClient.listLabels).mockResolvedValue([clients]);
    act(() => emit({ accountId: otherAccount, active: false }));
    await waitFor(() => expect(result.current.labelsByAccount[otherAccount]).toEqual([clients]));
    expect(result.current.labelsByAccount[account]).toEqual([inbox]);
    expect(mailClient.listLabels).toHaveBeenLastCalledWith(otherAccount);
  });

  it("retains cached labels on failure without looping, and retries at the next sync finish", async () => {
    const { result, rerender } = renderCatalog();
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox]));
    vi.mocked(mailClient.listLabels).mockRejectedValue(new Error("offline"));
    await act(async () => emit({ accountId: account, active: false }));
    rerender({ targetAccount: undefined });
    expect(result.current.labelsByAccount[account]).toEqual([inbox]);
    expect(mailClient.listLabels).toHaveBeenCalledTimes(2);

    vi.mocked(mailClient.listLabels).mockResolvedValue([inbox, clients]);
    act(() => emit({ accountId: account, active: false }));
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox, clients]));
  });

  it("ignores an older startup listing that resolves after the post-sync listing", async () => {
    const initial = deferred<Label[]>();
    vi.mocked(mailClient.listLabels).mockReturnValueOnce(initial.promise);
    const { result } = renderCatalog();
    await waitFor(() => expect(mailClient.listLabels).toHaveBeenCalledTimes(1));
    vi.mocked(mailClient.listLabels).mockResolvedValue([inbox, clients]);
    act(() => emit({ accountId: account, active: false }));
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox, clients]));

    await act(async () => initial.resolve([inbox]));
    expect(result.current.labelsByAccount[account]).toEqual([inbox, clients]);
  });

  it.each(["create", "rename", "delete"] as const)("does not let a pending listing undo a successful label %s", async (operation) => {
    vi.mocked(mailClient.listLabels).mockResolvedValue([inbox, clients]);
    vi.spyOn(mailClient, "createLabel").mockResolvedValue(projects);
    vi.spyOn(mailClient, "updateLabel").mockResolvedValue({ ...clients, name: "Customers" });
    vi.spyOn(mailClient, "deleteLabel").mockResolvedValue();
    const { result } = renderCatalog(account);
    await waitFor(() => expect(result.current.labelsByAccount[account]).toEqual([inbox, clients]));

    const stale = deferred<Label[]>();
    vi.mocked(mailClient.listLabels).mockReturnValueOnce(stale.promise);
    act(() => emit({ accountId: account, active: false }));
    await act(async () => {
      if (operation === "create") await result.current.createLabel(projects.name);
      if (operation === "rename") await result.current.renameLabel(clients.id, "Customers");
      if (operation === "delete") await result.current.deleteLabel(clients.id);
    });
    await act(async () => stale.resolve([inbox, clients]));

    const expected = operation === "create" ? [inbox, clients, projects]
      : operation === "rename" ? [inbox, { ...clients, name: "Customers" }] : [inbox];
    expect(result.current.labelsByAccount[account]).toEqual(expected);
  });

  it("stops listening and ignores pending listings after unmount", async () => {
    const listing = deferred<Label[]>();
    vi.mocked(mailClient.listLabels).mockReturnValueOnce(listing.promise);
    const { result, unmount } = renderCatalog();
    await waitFor(() => expect(mailClient.listLabels).toHaveBeenCalledTimes(1));
    unmount();
    await act(async () => listing.resolve([inbox]));
    expect(result.current.labelsByAccount).toEqual({});
    expect(unlisten).toHaveBeenCalledTimes(1);
  });
});
