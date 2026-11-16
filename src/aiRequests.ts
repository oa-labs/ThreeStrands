import { invoke } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";
import { aiPriceKey, readAiPrices } from "./aiSettings";
import { invokeWithPolicy, WAIT_FOR_NATIVE_COMPLETION } from "./invoke";

export type AiRequestPolicy = { dailyBudgetUsd: number | null };
export type AiSourceReference = { path: string; sourceId: string | null; revision: string; characters: number; truncated: boolean };
export type AiRequestRecord = {
  attemptId: string; requestId: string; at: string; feature: string; provider: string; model: string;
  promptVersion: number; schemaVersion: number; promptRevision: string; schemaRevision: string | null;
  context: { version: number; payloadRevision: string; scope: { accountIds: string[]; recordId: string | null; revision: string | null }; sources: AiSourceReference[] };
  responseRevision: string | null; outcome: string; requestOutcome: string; chargedUsd: number;
};
export type AiRequestSnapshot = { policy: AiRequestPolicy; day: string; chargedUsd: number; active: { requestId: string; feature: string }[]; records: AiRequestRecord[]; maxConcurrent: number; maxPending: number };
export type PendingAiRequest = { requestId: string; command: string; owner: string | null };
let pending: PendingAiRequest[] = [];
const listeners = new Set<() => void>();
const publish = () => listeners.forEach((listener) => listener());
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
export function usePendingAiRequests() { return useSyncExternalStore(subscribe, () => pending); }

/** Retain the native promise until cancellation is acknowledged; rejecting a
 * webview promise alone cannot abort a provider request. No payloads are kept. */
export async function invokeAi<T>(command: string, args: Record<string, unknown>, owner?: string): Promise<T> {
  const requestId = crypto.randomUUID();
  const price = readAiPrices()[aiPriceKey(args.provider as Parameters<typeof aiPriceKey>[0], String(args.model ?? ""))];
  const request = args.request as { threadId?: string; sourceMessageId?: string } | undefined;
  const target = owner ?? (args.threadId as string | undefined) ?? request?.threadId ?? request?.sourceMessageId ?? (args.id as string | undefined) ?? null;
  pending = [...pending, { requestId, command, owner: target }]; publish();
  try {
    return await invokeWithPolicy<T>(command, { ...args, control: { requestId, price: price ?? null } }, WAIT_FOR_NATIVE_COMPLETION);
  } finally {
    pending = pending.filter((request) => request.requestId !== requestId); publish();
  }
}
export function cancelAiRequest(requestId: string): Promise<void> { return invoke("cancel_ai_request", { requestId }); }
const demoSnapshot: AiRequestSnapshot = { policy: { dailyBudgetUsd: null }, day: "", chargedUsd: 0, active: [], records: [], maxConcurrent: 2, maxPending: 8 };
export function aiRequestSnapshot(includeHistory = false): Promise<AiRequestSnapshot> {
  return "__TAURI_INTERNALS__" in window ? invoke("ai_request_snapshot", { includeHistory }) : Promise.resolve(demoSnapshot);
}
export function setAiRequestPolicy(policy: AiRequestPolicy): Promise<void> {
  if ("__TAURI_INTERNALS__" in window) return invoke("set_ai_request_policy", { policy });
  demoSnapshot.policy = policy; return Promise.resolve();
}

export function recoverAiRequests(): Promise<void> { return invoke("recover_ai_requests"); }
