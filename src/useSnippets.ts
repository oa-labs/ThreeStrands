import { useCallback, useEffect, useState } from "react";
import { mailClient } from "./data/client";
import type { Snippet } from "./domain";
import { logBackgroundFailure } from "./errors";

/** Owns the reusable snippet library shared by Settings and the composer. */
export function useSnippets() {
  const [snippets, setSnippets] = useState<Snippet[]>([]);

  useEffect(() => {
    void mailClient.listSnippets().then(setSnippets).catch(logBackgroundFailure("Snippet listing"));
  }, []);

  const create = useCallback(async (name: string, body: string) => {
    const created = await mailClient.createSnippet(name, body);
    setSnippets((current) => [...current, created]);
    return created;
  }, []);

  const update = useCallback(async (id: string, name: string, body: string) => {
    const updated = await mailClient.updateSnippet(id, name, body);
    setSnippets((current) => current.map((snippet) => (snippet.id === id ? updated : snippet)));
    return updated;
  }, []);

  const remove = useCallback(async (id: string) => {
    await mailClient.deleteSnippet(id);
    setSnippets((current) => current.filter((snippet) => snippet.id !== id));
  }, []);

  return { snippets, create, update, remove };
}
