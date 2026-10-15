import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { mailClient } from "./data/client";
import { EMPTY_SIGNATURE_LIBRARY, type Signature, type SignatureDefaults, type SignatureLibrary, type SignatureState } from "./signatures";
import { logBackgroundFailure } from "./errors";

export function useSignatures(): SignatureState {
  const [library, setLibrary] = useState<SignatureLibrary>(EMPTY_SIGNATURE_LIBRARY);
  const requestGeneration = useRef(0);
  const refresh = useCallback(async () => {
    const generation = ++requestGeneration.current;
    const next = await mailClient.signatureLibrary();
    if (!next || !Array.isArray(next.signatures) || !Array.isArray(next.defaults)) throw new Error("Could not load signatures.");
    if (generation === requestGeneration.current) setLibrary(next);
  }, []);
  const cancelRefreshes = useCallback(() => { requestGeneration.current++; }, []);
  useEffect(() => {
    let active = true;
    let stop: (() => void) | undefined;
    const reload = () => void refresh().catch(logBackgroundFailure("Signature listing"));
    reload();
    if ("__TAURI_INTERNALS__" in window) void listen("replicated-sync-status", reload).then((unlisten) => { if (active) stop = unlisten; else unlisten(); }).catch(logBackgroundFailure("Signature sync listener"));
    return () => { active = false; cancelRefreshes(); stop?.(); };
  }, [refresh, cancelRefreshes]);
  const save = useCallback(async (signature: Signature) => { await mailClient.saveSignature(signature); await refresh(); }, [refresh]);
  const remove = useCallback(async (id: string) => { await mailClient.deleteSignature(id); await refresh(); }, [refresh]);
  const saveDefaults = useCallback(async (defaults: SignatureDefaults) => { await mailClient.saveSignatureDefaults(defaults); await refresh(); }, [refresh]);
  const inheritDefaults = useCallback(async (account: string, address: string) => { await mailClient.deleteSignatureDefaults(account, address); await refresh(); }, [refresh]);
  return { ...library, refresh, save, remove, saveDefaults, inheritDefaults };
}
