import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { Snippet } from "./domain";
import { useSnippets } from "./useSnippets";

const greeting: Snippet = { id: "snippet-1", name: "Greeting", body: "Hello", createdAt: "2026-09-01T00:00:00Z" };

describe("useSnippets", () => {
  afterEach(() => vi.restoreAllMocks());

  it("keeps the library in step with create, update, and delete", async () => {
    const signoff: Snippet = { id: "snippet-2", name: "Sign-off", body: "Thanks", createdAt: "2026-09-02T00:00:00Z" };
    vi.spyOn(mailClient, "listSnippets").mockResolvedValue([greeting]);
    vi.spyOn(mailClient, "createSnippet").mockResolvedValue(signoff);
    vi.spyOn(mailClient, "updateSnippet").mockResolvedValue({ ...greeting, body: "Hi there" });
    vi.spyOn(mailClient, "deleteSnippet").mockResolvedValue();
    const { result } = renderHook(() => useSnippets());
    await waitFor(() => expect(result.current.snippets).toEqual([greeting]));

    let created: Snippet | undefined;
    await act(async () => { created = await result.current.create("Sign-off", "Thanks"); });
    expect(created).toEqual(signoff);
    expect(result.current.snippets).toEqual([greeting, signoff]);

    await act(async () => { await result.current.update("snippet-1", "Greeting", "Hi there"); });
    expect(result.current.snippets[0]?.body).toBe("Hi there");

    await act(async () => { await result.current.remove("snippet-1"); });
    expect(result.current.snippets).toEqual([signoff]);
  });

  it("starts empty when the library cannot be listed", async () => {
    const listSnippets = vi.spyOn(mailClient, "listSnippets").mockRejectedValue(new Error("offline"));
    const { result } = renderHook(() => useSnippets());

    await waitFor(() => expect(listSnippets).toHaveBeenCalled());
    expect(result.current.snippets).toEqual([]);
  });
});
