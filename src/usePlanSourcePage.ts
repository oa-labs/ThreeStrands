import { useCallback, useEffect, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { PlanCandidate, PlanSourceKind } from "./domain";
import { errorMessage } from "./errors";

type PageState = { key: string; candidates: PlanCandidate[]; next: string | null; loading: boolean; error: string | null };

/**
 * One plan source's candidates from `planSources`, read a page at a time,
 * and only while `source` is set (the tab is showing). Changing the source,
 * the account, the date, or `refreshKey` starts again from the first page.
 */
export function usePlanSourcePage(date: string, source: PlanSourceKind | null, accountId: string | null, refreshKey: string) {
  const key = source ? JSON.stringify([date, source, accountId, refreshKey]) : "";
  const [state, setState] = useState<PageState>({ key: "", candidates: [], next: null, loading: false, error: null });
  const latest = useRef(key);
  latest.current = key;
  useEffect(() => {
    if (!source) return;
    let current = true;
    setState((previous) => ({ key, candidates: previous.key === key ? previous.candidates : [], next: null, loading: true, error: null }));
    mailClient.planSources(date, source, accountId, null)
      .then((page) => { if (current) setState({ key, candidates: page.candidates, next: page.next, loading: false, error: null }); })
      .catch((reason: unknown) => { if (current) setState({ key, candidates: [], next: null, loading: false, error: errorMessage(reason) }); });
    return () => { current = false; };
  }, [key, date, source, accountId]);
  const loadMore = useCallback(async () => {
    if (!source || state.key !== key || !state.next || state.loading) return;
    const cursor = state.next;
    setState((previous) => ({ ...previous, loading: true }));
    try {
      const page = await mailClient.planSources(date, source, accountId, cursor);
      if (latest.current !== key) return;
      setState((previous) => ({ ...previous, candidates: [...previous.candidates, ...page.candidates], next: page.next, loading: false }));
    } catch (reason) {
      if (latest.current !== key) return;
      setState((previous) => ({ ...previous, loading: false, error: errorMessage(reason) }));
    }
  }, [date, source, accountId, key, state]);
  const shown = state.key === key ? state : { candidates: [], next: null, loading: Boolean(source), error: null };
  return { candidates: shown.candidates, next: shown.next, loading: shown.loading, error: shown.error, loadMore };
}
