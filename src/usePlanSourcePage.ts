import { useCallback, useEffect, useRef, useState } from "react";
import { mailClient } from "./data/client";
import type { PlanCandidate, PlanSourceKind } from "./domain";
import { errorMessage } from "./errors";

type PageState = { key: string; context: string; pages: number; candidates: PlanCandidate[]; next: string | null; loading: boolean; error: string | null };

/**
 * One plan source's candidates from `planSources`, read a page at a time,
 * and only while `source` is set. Changing the source, account, or date starts
 * at the first page. With `retainOnRefresh`, refresh rereads the expanded page
 * count and keeps existing cards visible until those pages have been read.
 */
export function usePlanSourcePage(date: string, source: PlanSourceKind | null, accountId: string | null, refreshKey: string, retainOnRefresh = false) {
  const context = source ? JSON.stringify([date, source, accountId]) : "";
  const key = source ? JSON.stringify([date, source, accountId, refreshKey]) : "";
  const [state, setState] = useState<PageState>({ key: "", context: "", pages: 1, candidates: [], next: null, loading: false, error: null });
  const stateRef = useRef(state);
  stateRef.current = state;
  const latest = useRef(key);
  latest.current = key;
  useEffect(() => {
    if (!source) return;
    let current = true;
    const previous = stateRef.current;
    const keep = retainOnRefresh && previous.context === context;
    const pages = keep ? previous.pages : 1;
    setState({ key, context, pages, candidates: keep ? previous.candidates : [], next: keep ? previous.next : null, loading: true, error: null });
    const read = async () => {
      let cursor: string | null = null;
      const candidates: PlanCandidate[] = [];
      let count = 0;
      do {
        const page = await mailClient.planSources(date, source, accountId, cursor);
        if (!current) return;
        candidates.push(...page.candidates); cursor = page.next; count++;
      } while (cursor && count < pages);
      if (current) setState({ key, context, pages: count, candidates, next: cursor, loading: false, error: null });
    };
    void read().catch((reason: unknown) => {
      if (current) setState({ key, context, pages, candidates: keep ? previous.candidates : [], next: keep ? previous.next : null, loading: false, error: errorMessage(reason) });
    });
    return () => { current = false; };
  }, [key, context, date, source, accountId, retainOnRefresh]);
  const loadMore = useCallback(async () => {
    if (!source || state.key !== key || !state.next || state.loading) return;
    const cursor = state.next;
    setState((previous) => ({ ...previous, loading: true }));
    try {
      const page = await mailClient.planSources(date, source, accountId, cursor);
      if (latest.current !== key) return;
      setState((previous) => ({ ...previous, candidates: [...previous.candidates, ...page.candidates], pages: previous.pages + 1, next: page.next, loading: false }));
    } catch (reason) {
      if (latest.current !== key) return;
      setState((previous) => ({ ...previous, loading: false, error: errorMessage(reason) }));
    }
  }, [date, source, accountId, key, state]);
  const shown = state.key === key ? state
    : retainOnRefresh && Boolean(source) && state.context === context ? { ...state, loading: true, error: null }
      : { candidates: [], next: null, loading: Boolean(source), error: null };
  const remove = useCallback((candidateKey: string) => {
    setState((previous) => previous.context === context ? { ...previous, candidates: previous.candidates.filter((candidate) => candidate.key !== candidateKey) } : previous);
  }, [context]);
  return { candidates: shown.candidates, next: shown.next, loading: shown.loading, error: shown.error, loadMore, remove };
}
