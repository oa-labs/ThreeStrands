import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { mailClient } from "./data/client";
import type { MuteResult, SenderRule } from "./domain";
import { logBackgroundFailure } from "./errors";

/** Fired after a change made elsewhere, such as Mark Spam blocking a sender, so the list re-reads. */
export const SENDER_RULES_CHANGED = "threestrands:sender-rules-changed";

/** Owns the muted- and blocked-sender list shared by the context panel and Settings. */
export function useSenderRules() {
  const [senderRules, setSenderRules] = useState<SenderRule[]>([]);

  const refresh = useCallback(() => {
    return mailClient.listSenderRules()
      .then(setSenderRules)
      .catch(logBackgroundFailure("Muted sender listing"));
  }, []);

  useEffect(() => {
    const changed = () => void refresh();
    window.addEventListener(SENDER_RULES_CHANGED, changed);
    return () => window.removeEventListener(SENDER_RULES_CHANGED, changed);
  }, [refresh]);

  useEffect(() => {
    void refresh();
    // A rule muted or unmuted on another device arrives with device sync,
    // which only the desktop app has.
    if (!("__TAURI_INTERNALS__" in window)) return;
    let stop: (() => void) | undefined;
    let active = true;
    void listen("replicated-sync-status", () => void refresh())
      .then((unlisten) => { if (active) stop = unlisten; else unlisten(); })
      .catch(logBackgroundFailure("Muted sender sync listener"));
    return () => { active = false; stop?.(); };
  }, [refresh]);

  const mute = useCallback(async (accountId: string, matchKind: SenderRule["matchKind"], matchValue: string, archiveThreadIds: string[]): Promise<MuteResult> => {
    const result = await mailClient.muteSender(accountId, matchKind, matchValue, archiveThreadIds);
    setSenderRules((current) => current.some((rule) => rule.id === result.rule.id) ? current : [...current, result.rule]);
    return result;
  }, []);

  const unmute = useCallback(async (id: string) => {
    await mailClient.deleteSenderRule(id);
    setSenderRules((current) => current.filter((rule) => rule.id !== id));
  }, []);

  return { senderRules, refresh, mute, unmute };
}
