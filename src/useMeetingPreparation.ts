import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { MeetingContext, MeetingReadiness } from "./domain";
import { errorMessage } from "./errors";
import { noteReferenceKey, type NoteReference } from "./noteEntities";
import { useNoteChanges } from "./noteMutations";
import { MAX_READINESS_EVENTS } from "./meetingPrep";

type Loaded<T> = { key: string; value: T | null; loading: boolean; error: string | null };

/**
 * One meeting's preparation context, keyed by its occurrence and mail scope.
 * A result for a meeting the view has since left is ignored. The context
 * reloads when notes change here or on another device, or when `refreshKey`
 * changes (mail sync, task edits), keeping the last result on screen until
 * the new one arrives. The chosen scope belongs to the occurrence: opening
 * another meeting starts from its calendar's own account again.
 */
export function useMeetingPreparation(reference: NoteReference | null, refreshKey: unknown = null) {
  const key = reference ? JSON.stringify([noteReferenceKey(reference), reference.series ?? null]) : "";
  const [scope, setScopeState] = useState<{ key: string; accounts: string[] | null }>({ key, accounts: null });
  const accounts = scope.key === key ? scope.accounts : null;
  const requestKey = JSON.stringify([key, accounts]);
  const [state, setState] = useState<Loaded<MeetingContext>>({ key: "", value: null, loading: false, error: null });
  const [revision, setRevision] = useState(0);
  const version = useRef(0);
  // Read through refs: `requestKey` already names both.
  const referenceRef = useRef(reference);
  referenceRef.current = reference;
  const accountsRef = useRef(accounts);
  accountsRef.current = accounts;

  useEffect(() => {
    const current = referenceRef.current;
    if (!current) return;
    const mine = ++version.current;
    setState((previous) => ({ key: requestKey, value: previous.key === requestKey ? previous.value : null, loading: true, error: null }));
    mailClient.meetingContext(current, accountsRef.current).then(
      (value) => { if (version.current === mine) setState({ key: requestKey, value, loading: false, error: null }); },
      (reason) => { if (version.current === mine) setState((previous) => ({ ...previous, loading: false, error: errorMessage(reason) })); },
    );
  }, [requestKey, refreshKey, revision]);
  // Leaving the meeting, or unmounting, drops any answer still on its way.
  useEffect(() => () => { version.current++; }, [key]);
  useNoteChanges(useCallback(() => setRevision((current) => current + 1), []));

  const current = state.key === requestKey;
  return {
    context: current ? state.value : null,
    loading: !current || state.loading,
    error: current ? state.error : null,
    reload: useCallback(() => setRevision((value) => value + 1), []),
    /** The accounts chosen for this meeting, or null for its calendar's own. */
    mailScope: accounts,
    setMailScope: useCallback((next: string[] | null) => setScopeState({ key, accounts: next }), [key]),
  };
}

/** Readiness for up to `MAX_READINESS_EVENTS` meetings, such as today's. */
export function useMeetingReadiness(references: readonly NoteReference[], refreshKey: unknown = null) {
  const bounded = useMemo(() => references.slice(0, MAX_READINESS_EVENTS), [references]);
  const requestKey = JSON.stringify(bounded.map((reference) => [noteReferenceKey(reference), reference.series ?? null]));
  const [state, setState] = useState<Loaded<MeetingReadiness[]>>({ key: "", value: null, loading: false, error: null });
  const [revision, setRevision] = useState(0);
  const version = useRef(0);
  const boundedRef = useRef(bounded);
  boundedRef.current = bounded;
  useEffect(() => {
    const mine = ++version.current;
    const list = boundedRef.current;
    if (!list.length) { setState({ key: requestKey, value: [], loading: false, error: null }); return; }
    setState((previous) => ({ key: requestKey, value: previous.key === requestKey ? previous.value : null, loading: true, error: null }));
    mailClient.meetingReadiness(list).then(
      (value) => { if (version.current === mine) setState({ key: requestKey, value, loading: false, error: null }); },
      (reason) => { if (version.current === mine) setState((previous) => ({ ...previous, loading: false, error: errorMessage(reason) })); },
    );
  }, [requestKey, refreshKey, revision]);
  useEffect(() => () => { version.current++; }, []);
  useNoteChanges(useCallback(() => setRevision((current) => current + 1), []));
  const current = state.key === requestKey;
  return {
    rows: current ? state.value ?? [] : [],
    loading: !current || state.loading,
    error: current ? state.error : null,
    reload: useCallback(() => setRevision((value) => value + 1), []),
  };
}
