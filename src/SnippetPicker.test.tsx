import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Snippet } from "./domain";
import { SnippetEditor } from "./SnippetPicker";

describe("SnippetEditor", () => {
  afterEach(() => {
    cleanup();
    delete (window as { snippetEditorProbe?: boolean }).snippetEditorProbe;
  });

  function renderEditor(body: string, onUpdate = vi.fn().mockResolvedValue(undefined)) {
    const snippet = { id: "snippet-1", name: "Intro", body } as Snippet;
    render(
      <SnippetEditor target={snippet} initialName="Intro" onClose={() => {}} onBack={() => {}} onCreate={vi.fn()} onUpdate={onUpdate} />,
    );
    return { onUpdate, textarea: screen.getByRole("textbox", { name: "Body" }) as HTMLTextAreaElement };
  }

  it("edits an HTML snippet body as plain text with its line breaks and spacing", () => {
    const { textarea } = renderEditor("Hi {first_name},<br>  Thanks &amp; regards<div>Ada</div>");
    expect(textarea.value).toBe("Hi {first_name},\n  Thanks & regards\nAda");
  });

  it("does not run markup from a stored snippet body while loading it", () => {
    const { textarea } = renderEditor('<img src="x" onerror="window.snippetEditorProbe = true">Hello<script>window.snippetEditorProbe = true</script>');
    expect(textarea.value).toBe("Hello");
    expect((window as { snippetEditorProbe?: boolean }).snippetEditorProbe).toBeUndefined();
  });

  it("round-trips an unchanged body back to equivalent HTML", async () => {
    const { onUpdate } = renderEditor("Line one<br>  Line two");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onUpdate).toHaveBeenCalledWith("snippet-1", "Intro", "Line one<br>  Line two"));
  });
});
