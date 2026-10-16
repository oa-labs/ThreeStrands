import { describe, expect, it } from "vitest";
import type { Account, SyncStatus } from "./domain";
import { inboxCheckedLabel } from "./inboxZero";
import { formatMailTimestamp } from "./threadPresentation";

const now = new Date("2026-10-16T15:00:00");
const today = new Date("2026-10-16T14:42:00").toISOString();
const earlier = new Date("2026-10-14T09:00:00").toISOString();
const at = (iso: string) => formatMailTimestamp(iso, now);

const status = (overrides: Partial<SyncStatus> = {}): SyncStatus => ({
  state: "idle", lastSuccessfulSync: today, cursor: null, pendingMutations: 0, failedMutations: [],
  quarantinedMessages: [], error: null, fetchErrors: [], ...overrides,
});
const account = (email: string, overrides: Partial<Account> = {}) => ({
  email, status: "connected" as Account["status"], lastSyncedAt: today, ...overrides,
});
const text = (...args: Parameters<typeof inboxCheckedLabel>) => inboxCheckedLabel(...args)?.text ?? null;

describe("inbox zero checked label", () => {
  it("names the last check of the account in view", () => {
    expect(text(status(), [account("me@example.com")], now)).toBe(`Last checked ${at(today)}`);
  });

  it("uses the account's own check, not a newer one from another account", () => {
    const viewed = account("imap@example.com", { lastSyncedAt: earlier });
    expect(text(status({ lastSuccessfulSync: today }), [viewed], now)).toBe(`Last checked ${at(earlier)}`);
  });

  it("reports the oldest check when several accounts are in view", () => {
    expect(text(status(), [account("a@example.com"), account("b@example.com", { lastSyncedAt: earlier })], now))
      .toBe(`Last checked ${at(earlier)}`);
  });

  it("ignores another account's failure and failed queued actions", () => {
    const other = status({
      state: "error",
      error: "other@example.com: token expired",
      fetchErrors: [{ accountId: "other@example.com", error: "token expired" }],
      failedMutations: [{ id: "1", kind: "archive", threadId: "t", attempts: 3, error: "rejected", createdAt: today }],
    });
    expect(text(other, [account("me@example.com")], now)).toBe(`Last checked ${at(today)}`);
  });

  it("says when the account in view could not be checked, with its last good check", () => {
    const failing = status({ state: "error", fetchErrors: [{ accountId: "me@example.com", error: "timed out" }] });
    expect(text(failing, [account("me@example.com")], now)).toBe(`Couldn't check for new mail · Last checked ${at(today)}`);
    expect(text(failing, [account("me@example.com"), account("you@example.com")], now))
      .toBe(`Couldn't check me@example.com for new mail · Last checked ${at(today)}`);
  });

  it("asks to reconnect an account that needs signing in again", () => {
    expect(inboxCheckedLabel(status(), [account("me@example.com", { status: "needs_reauth" })], now))
      .toEqual({ text: "Reconnect this account to check for new mail", needsReconnect: true });
    expect(text(status(), [account("a@example.com"), account("b@example.com", { status: "needs_reauth" })], now))
      .toBe("Reconnect b@example.com to check for new mail");
  });

  it("says when mail is being checked or the device is offline", () => {
    expect(text(status({ state: "syncing" }), [account("me@example.com")], now)).toBe("Checking for new mail…");
    expect(text(status({ state: "offline" }), [account("me@example.com")], now)).toBe(`Offline · Last checked ${at(today)}`);
  });

  it("says nothing when there is no check to report", () => {
    expect(inboxCheckedLabel(status(), [account("me@example.com", { lastSyncedAt: null })], now)).toBeNull();
    expect(inboxCheckedLabel(null, [], now)).toBeNull();
  });
});
