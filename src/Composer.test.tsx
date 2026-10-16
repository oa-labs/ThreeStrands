import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createRef } from "react";
import DOMPurify from "dompurify";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Composer, type ComposerHandle } from "./Composer";
import type { Draft, OutboxItem } from "./correspondence";
import { mailClient } from "./data/client";
import type { Account, Snippet } from "./domain";
import { draftSignatureSnapshot, sanitizeSignatureHtml, signatureBody, type SignatureState } from "./signatures";
import {
  clearAiApiKey,
  DEFAULT_AI_FEATURES,
  saveAiFeatures,
  saveAiProvider,
  setAiApiKey,
} from "./aiSettings";

const draft: Draft = {
  id: "draft-1",
  revision: 0,
  account: "first@example.com",
  mode: "new",
  sourceId: null,
  threadId: null,
  replyId: null,
  references: [],
  to: "",
  cc: "",
  bcc: "",
  subject: "",
  body: "",
  attachments: [],
  updatedAt: 0,
};

const accounts: Account[] = ["first@example.com", "second@example.com"].map((email, sortOrder) => ({
  email,
  displayName: null,
  color: "#4285F4",
  status: "connected",
  provider: "gmail",
  labelModel: "gmailLabels",
  sortOrder,
  connectedAt: "2026-01-01T00:00:00Z",
  lastSyncedAt: null,
  onThisDevice: true,
}));

const snippetProps = {
  snippets: [] as Snippet[],
  onCreateSnippet: vi.fn(),
  onUpdateSnippet: vi.fn(),
  onDeleteSnippet: vi.fn(),
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

describe("Composer signatures", () => {
  const full = { id: "full", name: "Full", html: "<p>Alex Example</p>", text: "Alex Example" };
  const short = { id: "short", name: "Short", html: "<p>Alex</p>", text: "Alex" };
  const signatures: SignatureState = { signatures: [full, short], defaults: [], refresh: vi.fn(), save: vi.fn(), remove: vi.fn(), saveDefaults: vi.fn(), inheritDefaults: vi.fn() };
  beforeEach(() => {
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (value) => ({ ...value, revision: value.revision + 1 }));
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: draft.account }]);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });
  it.each(["new", "reply", "replyAll", "forward"] as const)("keeps signature management below the editor without changing the %s draft", async (mode) => {
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={{ ...draft, mode, body: "Keep this message", signature: draftSignatureSnapshot(full, true) }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const picker = screen.getByRole("combobox", { name: "Signature" });
    const body = screen.getByRole("textbox", { name: "Message Body" });
    expect(body.compareDocumentPosition(picker) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Manage Signatures/i })).not.toBeInTheDocument();
    fireEvent.change(picker, { target: { value: "__manage_signatures__" } });
    const manager = screen.getByRole("dialog", { name: "Manage Signatures" });
    expect(within(manager).getByRole("button", { name: "Add Signature" })).toBeInTheDocument();
    expect(picker).toHaveValue("full");
    fireEvent.click(within(manager).getByRole("button", { name: "Close" }));
    let saved!: Draft;
    await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.signature?.id).toBe("full");
    expect(saved.body).toBe("Keep this message");
  });

  it("keeps the signature separate from authored text and quotes, including AI replacement and draft reopening", async () => {
    const ref = createRef<ComposerHandle>();
    const initial = { ...draft, mode: "reply" as const, body: "\n\nOn Monday, Ann wrote:\n> Quoted", signature: draftSignatureSnapshot(full, true) };
    const view = render(<Composer ref={ref} draft={initial} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const body = screen.getByRole("textbox", { name: "Message Body" });
    const signature = screen.getByRole("textbox", { name: "Message Signature" });
    const quote = document.querySelector('[aria-label="Quoted Text"]')!;
    expect(body.compareDocumentPosition(signature) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(signature.compareDocumentPosition(quote) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    let saved!: Draft;
    await act(async () => {
      const review = ref.current!.draftReview.readDraft();
      expect(review.body).not.toContain("Alex Example");
      ref.current!.draftReview.replaceDraft(review, { subject: "Reply", body: "Hi 😀" });
      saved = await ref.current!.flush();
    });
    expect(signature).toHaveTextContent("Alex Example");
    expect(signatureBody(saved).text).toMatch(/Hi 😀[\s\S]*Alex Example[\s\S]*On Monday/);
    expect(saved.signature?.automatic).toBe(true);
    expect(saved.signature?.textOffset).toBe(5);
    view.unmount();
    render(<Composer draft={saved} accounts={accounts} signatures={{ ...signatures, signatures: [] }} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    expect(screen.getAllByRole("textbox", { name: "Message Signature" })).toHaveLength(1);
    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveTextContent("Hi 😀");
    expect(screen.getByRole("textbox", { name: "Message Body" })).not.toHaveTextContent("Alex Example");
    expect(screen.getByRole("combobox", { name: "Signature" })).toHaveValue("full");
  });

  it("replaces and removes signatures without appending or changing the written message", async () => {
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={{ ...draft, body: "Written message", signature: draftSignatureSnapshot(full, true) }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Signature" }), { target: { value: "short" } });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message Signature" })).toHaveTextContent(/^Alex$/));
    let saved!: Draft; await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.body).toBe("Written message"); expect(saved.signature?.automatic).toBe(false);
    expect(signatureBody(saved).text).toBe("Written message\n\nAlex");
    fireEvent.change(screen.getByRole("combobox", { name: "Signature" }), { target: { value: "" } });
    await waitFor(() => expect(screen.queryByRole("textbox", { name: "Message Signature" })).toBeNull());
    await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.signature).toEqual(expect.objectContaining({ id: null, automatic: false, html: "" }));
    expect(saved.body).toBe("Written message");
  });

  it.each(["reply", "replyAll"] as const)("hides the picker for a %s with no applied signature", (mode) => {
    render(<Composer draft={{ ...draft, mode, signature: draftSignatureSnapshot(null, true) }} accounts={accounts} signatures={{ ...signatures, defaults: [{ id: "account", account: draft.account, address: "", newSignatureId: full.id, replySignatureId: null }] }} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    expect(screen.queryByRole("combobox", { name: "Signature" })).toBeNull();
    expect(screen.queryByRole("group", { name: "Writing tools" })).toBeNull();
    expect(screen.queryByRole("textbox", { name: "Message Signature" })).toBeNull();
  });

  it.each(["new", "forward"] as const)("retains the picker for %s messages with no applied signature", (mode) => {
    render(<Composer draft={{ ...draft, mode, signature: draftSignatureSnapshot(null, true) }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    expect(screen.getByRole("combobox", { name: "Signature" })).toHaveValue("");
  });

  it.each([
    '<table><tr><td><img src="https://images.example/logo.png" width="32" alt="Logo"></td><td>Alex</td><td></td></tr></table>',
    '<div style="display:flex"><img src="https://images.example/logo.png" alt="Logo" height="32"><p>Alex</p></div>',
  ])("loads only the user's selected signature images, keeping layout and automatic defaults intact: %s", async (html) => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue("data:image/png;base64,aA==");
    const ref = createRef<ComposerHandle>();
    const canonical = sanitizeSignatureHtml(html);
    const snapshot = draftSignatureSnapshot({ ...full, html: canonical, text: "Custom plain text" }, true);
    render(<Composer ref={ref} draft={{ ...draft, mode: "reply", signature: snapshot, bodyHtml: '<p>Hello<img src="https://images.example/body.png"></p>' }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const root = screen.getByRole("textbox", { name: "Message Signature" });
    await waitFor(() => expect(root.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,aA=="));
    expect(root.querySelector("button,[role=slider],[data-compose-image]")).toBeNull();
    expect(fetch.mock.calls).toEqual([["https://images.example/logo.png"]]);
    let saved!: Draft; await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.signature).toEqual(expect.objectContaining({ html: canonical, text: "Custom plain text", automatic: true }));
  });

  it.each(["data:image/svg+xml;base64,PHN2Zz4=", "https://images.example/redirect.png"])("keeps unsupported proxy results blocked in the signature: %s", async (data) => {
    vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue(data);
    const snapshot = draftSignatureSnapshot({ ...full, html: '<p>Alex<img src="https://images.example/logo.png"></p>' }, true);
    render(<Composer draft={{ ...draft, signature: snapshot }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("did not return a supported image");
    expect(screen.getByRole("textbox", { name: "Message Signature" }).querySelector("img")).not.toHaveAttribute("src");
  });

  it("keeps the signature above plain forwarded history when a written draft is reopened", async () => {
    const ref = createRef<ComposerHandle>();
    const initial = { ...draft, mode: "forward" as const, body: "\n\n---------- Forwarded message ----------\nFrom: Ann <ann@example.com>\n\nOriginal plain message", signature: draftSignatureSnapshot(full, true) };
    const view = render(<Composer ref={ref} draft={initial} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const body = screen.getByRole("textbox", { name: "Message Body" }); body.innerHTML = "Take a look"; fireEvent.input(body);
    let saved!: Draft; await act(async () => { saved = await ref.current!.flush(); });
    view.unmount();
    render(<Composer ref={ref} draft={saved} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveTextContent(/^Take a look$/);
    expect(document.querySelector('[aria-label="Quoted Text"]')).toHaveTextContent("Original plain message");
    await act(async () => { saved = await ref.current!.flush(); });
    expect(signatureBody(saved).text).toMatch(/Take a look[\s\S]*Alex Example[\s\S]*Forwarded message[\s\S]*Original plain message/);
  });

  it("snapshots inline signature edits without modifying the saved template", async () => {
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={{ ...draft, signature: draftSignatureSnapshot(full, true) }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const editor = screen.getByRole("textbox", { name: "Message Signature" });
    editor.innerHTML = "<b>Cheers, Alex</b>"; fireEvent.input(editor);
    let saved!: Draft; await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.signature).toEqual(expect.objectContaining({ html: "<b>Cheers, Alex</b>", text: "Cheers, Alex", automatic: false }));
    expect(signatures.save).not.toHaveBeenCalled(); expect(full.text).toBe("Alex Example");
  });

  it("preserves custom signature plain text through composing and draft reopening", async () => {
    const ref = createRef<ComposerHandle>();
    const snapshot = draftSignatureSnapshot({ ...full, html: "<p>Alex Example</p>", text: "Alex — custom plain text" }, true);
    const view = render(<Composer ref={ref} draft={{ ...draft, signature: snapshot }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const body = screen.getByRole("textbox", { name: "Message Body" }); body.innerHTML = "Hello"; fireEvent.input(body);
    let saved!: Draft; await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.signature?.text).toBe(snapshot.text); expect(saved.signature?.automatic).toBe(true);
    view.unmount();
    render(<Composer ref={ref} draft={saved} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const visual = screen.getByRole("textbox", { name: "Message Signature" }); visual.innerHTML = "<p>Alex Updated</p>"; fireEvent.input(visual);
    await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.signature?.text).toBe(snapshot.text); expect(saved.signature?.automatic).toBe(false);
    expect(signatureBody(saved).text).toContain("Alex — custom plain text");
  });

  it("automatically previews hosted signature images and preserves their URLs through draft saving and reopening", async () => {
    const ref = createRef<ComposerHandle>();
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue("data:image/png;base64,aA==");
    const snapshot = draftSignatureSnapshot({ ...full, html: '<p>Alex<img src="https://images.example/large.png" alt="Logo" width="120"></p>', text: "AlexLogo" }, true);
    const view = render(<Composer ref={ref} draft={{ ...draft, signature: snapshot }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const editor = screen.getByRole("textbox", { name: "Message Signature" });
    await waitFor(() => expect(editor.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,aA=="));
    expect(editor.querySelector("[data-compose-image]" )).toBeNull();
    let saved!: Draft; await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.signature?.html).toContain('src="https://images.example/large.png"');
    expect(saved.signature?.html).not.toMatch(/data-compose|<button/);
    expect(saved.signature?.text).toBe("AlexLogo");
    expect(saved.signature?.automatic).toBe(true);
    view.unmount();
    render(<Composer ref={ref} draft={saved} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message Signature" }).querySelector("img")).toHaveAttribute("src", "data:image/png;base64,aA=="));
    let reopened!: Draft; await act(async () => { reopened = await ref.current!.flush(); });
    expect(reopened.signature?.html).toContain('src="https://images.example/large.png"');
    expect(reopened.signature?.automatic).toBe(true);
    expect(fetch).toHaveBeenCalledWith("https://images.example/large.png");
  });

  it("resolves a signature's pasted image after reopening and keeps its native CID when saving", async () => {
    const ref = createRef<ComposerHandle>();
    const read = vi.spyOn(mailClient, "readInlineImage").mockResolvedValue("data:image/png;base64,aA==");
    const snapshot = draftSignatureSnapshot({ ...full, html: '<p>Alex<img src="cid:logo@local" alt="Logo" width="24"></p>', text: "AlexLogo" }, true);
    render(<Composer ref={ref} draft={{ ...draft, signature: snapshot, attachments: [{ id: "inline", name: "logo.png", mime: "image/png", size: 1, ready: true, inline: true, contentId: "logo@local", messageId: null, providerId: null }] }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const editor = screen.getByRole("textbox", { name: "Message Signature" });
    await waitFor(() => expect(editor.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,aA=="));
    expect(read).toHaveBeenCalledWith(draft.id, "inline");
    let saved!: Draft; await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.signature?.html).toContain('src="cid:logo@local"');
    expect(saved.signature?.html).not.toMatch(/data-compose|<button|data:image/);
    expect(saved.signature?.text).toBe("AlexLogo");
  });

  it("allows deleting a signature image as a per-message edit without overlay controls", async () => {
    const ref = createRef<ComposerHandle>();
    const snapshot = draftSignatureSnapshot({ ...full, html: '<p>Alex<img src="data:image/png;base64,aA==" alt="Logo"></p>', text: "AlexLogo" }, true);
    render(<Composer ref={ref} draft={{ ...draft, signature: snapshot }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const editor = screen.getByRole("textbox", { name: "Message Signature" });
    expect(within(editor).queryByRole("button", { name: "Remove Pasted Image" })).toBeNull();
    editor.querySelector("img")!.remove(); fireEvent.input(editor);
    let saved!: Draft; await act(async () => { saved = await ref.current!.flush(); });
    expect(saved.signature).toEqual(expect.objectContaining({ html: "<p>Alex</p>", text: "Alex", automatic: false }));
  });

  it("reflects the native automatic signature when changing From and preserves body edits", async () => {
    const ref = createRef<ComposerHandle>();
    vi.spyOn(mailClient, "setDraftAccount").mockImplementation(async (_id, account) => ({ ...draft, account, body: "My message", bodyHtml: "My message", signature: draftSignatureSnapshot(short, true), revision: 3 }));
    render(<Composer ref={ref} draft={{ ...draft, body: "My message", signature: draftSignatureSnapshot(full, true) }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Send From" }), { target: { value: accounts[1].email } });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Message Signature" })).toHaveTextContent(/^Alex$/));
    await act(async () => { await ref.current!.flush(); });
    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveTextContent("My message");
  });

  it.each([true, false])("closing a signature-only draft discards it only when automatic=%s", async (automatic) => {
    const discard = vi.spyOn(mailClient, "discardDraft").mockResolvedValue(undefined);
    const onClose = vi.fn();
    render(<Composer draft={{ ...draft, signature: draftSignatureSnapshot(full, automatic) }} accounts={accounts} signatures={signatures} {...snippetProps} onClose={onClose} onQueued={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Save and Close Draft" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(discard).toHaveBeenCalledTimes(automatic ? 1 : 0);
  });
});

describe("Composer From selector", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("keeps native pointer interaction and changes the sending account inline", async () => {
    vi.spyOn(mailClient, "setDraftAccount").mockResolvedValue({
      ...draft,
      revision: 1,
      account: "second@example.com",
    });

    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const selector = screen.getByRole("combobox", { name: "Send From" });

    expect(fireEvent.pointerDown(selector, { button: 0, pointerId: 1 })).toBe(true);
    fireEvent.change(selector, { target: { value: "second@example.com" } });

    await waitFor(() => expect(selector).toHaveValue("second@example.com"));
    expect(mailClient.setDraftAccount).toHaveBeenCalledWith("draft-1", "second@example.com");
  });

  it("keeps the From selector inside the composer's keyboard focus loop", () => {
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const selector = screen.getByRole("combobox", { name: "Send From" });

    selector.focus();
    fireEvent.keyDown(selector, { key: "Tab", shiftKey: true });

    expect(screen.getByRole("button", { name: "Discard Draft" })).toHaveFocus();
  });

  it("gives a new message a taller body than a reply", () => {
    const { unmount } = render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.getByRole("dialog", { name: "New Message" })).toHaveClass("composer-new");
    unmount();
    render(<Composer draft={{ ...draft, mode: "reply" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.getByRole("dialog", { name: "Reply Message" })).not.toHaveClass("composer-new");

    const css = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");
    expect(css).toContain(".composer-new .compose-body:not(.compose-quoted):not(.compose-signature) { height: min(45vh + 90px, 600px, 100vh - 400px); }");
  });
});

describe("Composer recipient visibility and shortcuts", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("shows populated copy fields by default and toggles only blank fields from the To label", () => {
    render(<Composer draft={{ ...draft, cc: "copy@example.com" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    expect(screen.getByRole("textbox", { name: "Cc" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Bcc" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cc / Bcc" })).not.toBeInTheDocument();

    const toLabel = screen.getByRole("button", { name: "To" });
    expect(toLabel).toHaveAttribute("title", "Click to show Cc/Bcc");

    fireEvent.click(toLabel);
    expect(screen.getByRole("textbox", { name: "Bcc" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "To" })).toHaveAttribute("title", "Click to hide Cc/Bcc");

    fireEvent.click(screen.getByRole("button", { name: "To" }));
    expect(screen.getByRole("textbox", { name: "Cc" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Bcc" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "To" })).toHaveAttribute("title", "Click to show Cc/Bcc");
  });

  it.each([
    ["o", "To"],
    ["c", "Cc"],
    ["b", "Bcc"],
  ])("focuses the %s recipient using its shortcut", (key, label) => {
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    fireEvent.keyDown(screen.getByRole("dialog"), { key, metaKey: true, shiftKey: true });

    expect(screen.getByRole("textbox", { name: label })).toHaveFocus();
  });
});

describe("Composer formatting hints", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    window.getSelection()?.removeAllRanges();
  });

  const select = (node: Node, start: number, end: number) => {
    const range = document.createRange();
    range.setStart(node, start);
    range.setEnd(node, end);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    act(() => { document.dispatchEvent(new Event("selectionchange")); });
  };

  it("trades the undo reminder for formatting shortcuts only while body text is selected", () => {
    render(<Composer draft={{ ...draft, body: "Hello there" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    expect(screen.getByText(/Send has a 10-second undo window/)).toBeInTheDocument();

    select(editor.firstChild!, 0, 5);
    const hints = screen.getByLabelText("Formatting shortcuts");
    expect(hints).toHaveTextContent("Bold Ctrl B");
    expect(hints).toHaveTextContent("Italics Ctrl I");
    expect(hints).toHaveTextContent("Strikethrough Ctrl Shift X");
    expect(screen.queryByText(/Send has a 10-second undo window/)).not.toBeInTheDocument();

    select(editor.firstChild!, 2, 2);
    expect(screen.queryByLabelText("Formatting shortcuts")).not.toBeInTheDocument();
    expect(screen.getByText(/Send has a 10-second undo window/)).toBeInTheDocument();
  });

  it("asks for a link in-app, since the desktop webview has no window.prompt", () => {
    const execute = vi.fn(() => true);
    Object.defineProperty(document, "execCommand", { configurable: true, value: execute });
    const nativePrompt = vi.spyOn(window, "prompt");
    render(<Composer draft={{ ...draft, body: "Hello there" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    const text = editor.firstChild!;

    select(text, 0, 5);
    fireEvent.keyDown(editor, { key: "k", metaKey: true });
    fireEvent.click(within(screen.getByRole("dialog", { name: "Insert Link" })).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog", { name: "Insert Link" })).not.toBeInTheDocument();
    expect(execute).not.toHaveBeenCalled();

    select(text, 0, 5);
    fireEvent.keyDown(editor, { key: "k", metaKey: true });
    const dialog = screen.getByRole("dialog", { name: "Insert Link" });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Link URL" }), { target: { value: "example.com" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Insert Link" }));

    expect(execute).toHaveBeenCalledWith("createLink", false, "https://example.com");
    const range = window.getSelection()!.getRangeAt(0);
    expect([range.startContainer, range.startOffset, range.endOffset]).toEqual([text, 0, 5]);
    expect(nativePrompt).not.toHaveBeenCalled();
  });

  it("keeps the undo reminder for selections outside the message body", () => {
    render(<Composer draft={{ ...draft, subject: "Status", body: "Hello there" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    select(screen.getByText(/Send has a 10-second undo window/).firstChild!, 0, 6);
    expect(screen.queryByLabelText("Formatting shortcuts")).not.toBeInTheDocument();
  });
});

describe("Composer list marker shortcuts", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it.each([
    ["*", "ul > li"],
    ["1.", "ol > li"],
  ])("starts a list when space follows %s", (marker, selector) => {
    render(<Composer draft={{ ...draft, body: marker }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    const text = editor.firstChild!;
    const range = document.createRange();
    range.setStart(text, marker.length);
    range.collapse(true);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);

    fireEvent.keyDown(editor, { key: " " });

    expect(editor.querySelector(selector)).toBeInTheDocument();
    expect(editor).not.toHaveTextContent(marker);
  });
});

describe("Composer pasted links", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  const renderWithSelection = (collapsed: boolean, quoted = false) => {
    const initial = quoted
      ? { ...draft, mode: "reply" as const, bodyHtml: 'Answer<br><br>On Monday, Sender wrote:<blockquote type="cite">Read the docs</blockquote>' }
      : { ...draft, body: "Read the docs" };
    render(<Composer draft={initial} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    if (quoted) fireEvent.click(screen.getByRole("button", { name: "Show Quoted Text" }));
    const editor = screen.getByRole("textbox", { name: quoted ? "Quoted Text" : "Message Body" });
    const text = quoted ? editor.querySelector("blockquote")!.firstChild! : editor.firstChild!;
    const range = document.createRange();
    range.setStart(text, 9);
    range.setEnd(text, collapsed ? 9 : 13);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
    // Simulate a browser that performs the edit without emitting `input`.
    const execute = vi.fn((command: string, _ui: boolean, value: string) => {
      const range = selection.getRangeAt(0);
      if (command === "createLink") {
        const link = document.createElement("a");
        link.href = value;
        link.append(range.extractContents());
        range.insertNode(link);
      } else {
        range.deleteContents();
        range.insertNode(range.createContextualFragment(value));
      }
      return true;
    });
    Object.defineProperty(document, "execCommand", { configurable: true, value: execute });
    return { editor, execute };
  };

  it("links the selected text and saves without a native input event", async () => {
    const { editor, execute } = renderWithSelection(false);

    fireEvent.paste(editor, { clipboardData: { files: [], getData: () => "https://example.com/docs" } });

    expect(execute).toHaveBeenCalledWith("createLink", false, "https://example.com/docs");
    expect(execute).not.toHaveBeenCalledWith("insertHTML", expect.anything(), expect.anything());
    expect(editor.querySelector("a")).toHaveTextContent("docs");
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    expect(mailClient.saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      body: "Read the docs",
      bodyHtml: 'Read the <a href="https://example.com/docs">docs</a>',
    }));
  });

  it("inserts and saves a pasted URL as a link when nothing is selected", async () => {
    const { editor, execute } = renderWithSelection(true);

    fireEvent.paste(editor, { clipboardData: { files: [], getData: () => "https://example.com/docs" } });

    expect(execute).toHaveBeenCalledWith("insertHTML", false, '<a href="https://example.com/docs">https://example.com/docs</a>');
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    expect(mailClient.saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      bodyHtml: 'Read the <a href="https://example.com/docs">https://example.com/docs</a>docs',
    }));
  });

  it("replaces and saves the selection with pasted text that is not a lone URL", async () => {
    const { editor, execute } = renderWithSelection(false);

    fireEvent.paste(editor, { clipboardData: { files: [], getData: () => "see https://example.com/docs" } });

    expect(execute).not.toHaveBeenCalledWith("createLink", expect.anything(), expect.anything());
    expect(execute).toHaveBeenCalledWith("insertHTML", false, 'see <a href="https://example.com/docs">https://example.com/docs</a>');
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    expect(mailClient.saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      body: "Read the see https://example.com/docs",
      bodyHtml: 'Read the see <a href="https://example.com/docs">https://example.com/docs</a>',
    }));
  });

  it.each([false, "throw"])("reports a failed paste (%s) without marking the draft saved", async (failure) => {
    const { editor, execute } = renderWithSelection(false);
    execute.mockImplementation(() => {
      if (failure === "throw") throw new Error("Command unavailable");
      return false;
    });
    fireEvent.paste(editor, { clipboardData: { files: [], getData: () => "https://example.com/docs" } });
    expect(screen.getByRole("alert")).toHaveTextContent(/paste/i);
    expect(editor).toHaveTextContent("Read the docs");
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    expect(mailClient.saveDraft).not.toHaveBeenCalled();
  });

  it("saves pasted markup as text and rejects unsafe link destinations", async () => {
    const { editor } = renderWithSelection(false);
    fireEvent.paste(editor, { clipboardData: { files: [], getData: () => '<img src=x onerror=alert(1)> javascript:alert(1)' } });
    expect(editor.querySelector("img, a, script")).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    expect(mailClient.saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      body: 'Read the <img src=x onerror=alert(1)> javascript:alert(1)',
      bodyHtml: 'Read the &lt;img src=x onerror=alert(1)&gt; javascript:alert(1)',
    }));
  });

  it("persists a link pasted into quoted history without changing the authored body", async () => {
    const { editor } = renderWithSelection(false, true);
    fireEvent.paste(editor, { clipboardData: { files: [], getData: () => "https://example.com/docs" } });
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveTextContent("Answer");
    expect(mailClient.saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      body: "Answer\n\nOn Monday, Sender wrote:\n> Read the docs",
      bodyHtml: expect.stringMatching(/^Answer<br><br>On Monday, Sender wrote:<blockquote type="cite"[^>]*>Read the <a href="https:\/\/example.com\/docs">docs<\/a><\/blockquote>$/),
    }));
  });
});

describe("Composer body input responsiveness", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("defers cloning and sanitizing the message body until the autosave boundary", async () => {
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    const cloneNode = vi.spyOn(editor, "cloneNode");

    for (const text of ["A", "A longer", "A longer message"]) {
      editor.innerHTML = text;
      fireEvent.input(editor);
    }

    expect(cloneNode).not.toHaveBeenCalled();
    expect(saveDraft).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(300);

    // One copy of the body per autosave: HTML and plain text share it.
    expect(cloneNode).toHaveBeenCalledTimes(1);
    expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      body: "A longer message",
      bodyHtml: "A longer message",
    }));
  });

  it("does not re-sanitize the quoted thread when the composer re-renders during input", async () => {
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const quoted = Array.from({ length: 200 }, (_, index) => `> Quoted line ${index}`).join("\n");
    const reply: Draft = { ...draft, mode: "replyAll", to: "a@example.com", body: `\n\nOn Monday, A wrote:\n${quoted}` };
    const props = { draft: reply, accounts, ...snippetProps, onClose: () => {}, onQueued: () => {} };
    const { rerender } = render(<Composer {...props} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    expect(screen.getByLabelText("Quoted Text").textContent).toContain("Quoted line 199");
    const sanitize = vi.spyOn(DOMPurify, "sanitize");

    // The first input flips the status to "Unsaved changes", and a parent
    // re-render (e.g. App state) renders the composer again. Neither may do
    // work proportional to the quoted thread before the text is painted.
    editor.insertAdjacentText("afterbegin", "Dictated reply. ");
    fireEvent.input(editor);
    rerender(<Composer {...props} />);

    expect(screen.getByRole("status")).toHaveTextContent("Unsaved changes");
    expect(sanitize).not.toHaveBeenCalled();
    expect(editor.textContent).toMatch(/^Dictated reply\. /);
  });

  it("captures the latest body immediately when a send or close flushes the draft", async () => {
    const ref = createRef<ComposerHandle>();
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer ref={ref} draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });

    editor.innerHTML = "<strong>Send this text</strong>";
    fireEvent.input(editor);
    await ref.current?.flush();

    expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      body: "Send this text",
      bodyHtml: "<strong>Send this text</strong>",
    }));
  });

  it("serializes saves and makes concurrent flushes wait for edits made during saving", async () => {
    const first = deferred<Draft>();
    const second = deferred<Draft>();
    const saveDraft = vi.spyOn(mailClient, "saveDraft")
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    editor.innerHTML = "First version";
    fireEvent.input(editor);
    let firstFlush!: Promise<Draft>;
    act(() => { firstFlush = ref.current!.flush(); });

    editor.innerHTML = "<b>Latest version</b>";
    fireEvent.input(editor);
    fireEvent.change(screen.getByRole("textbox", { name: "Subject" }), { target: { value: "Latest subject" } });
    const secondFlush = ref.current!.flush();
    const thirdFlush = ref.current!.flush();
    const finished = vi.fn();
    void Promise.all([firstFlush, secondFlush, thirdFlush]).then(finished);
    expect(saveDraft).toHaveBeenCalledTimes(1);

    await act(async () => { first.resolve({ ...saveDraft.mock.calls[0][0], revision: 1, updatedAt: 10 }); });
    expect(saveDraft).toHaveBeenCalledTimes(2);
    expect(saveDraft.mock.calls[1][0]).toMatchObject({
      revision: 1, updatedAt: 10, subject: "Latest subject", body: "Latest version", bodyHtml: "<b>Latest version</b>",
    });
    expect(finished).not.toHaveBeenCalled();

    await act(async () => { second.resolve({ ...saveDraft.mock.calls[1][0], revision: 2, updatedAt: 20 }); });
    const results = await Promise.all([firstFlush, secondFlush, thirdFlush]);
    expect(results.every((saved) => saved.revision === 2 && saved.body === "Latest version")).toBe(true);
    expect(screen.getByRole("status")).toHaveTextContent("Saved on this device");
    await ref.current!.flush();
    expect(saveDraft).toHaveBeenCalledTimes(2);
  });

  it("keeps failed saves dirty, blocks closing, and retries with the latest edits", async () => {
    const first = deferred<Draft>();
    const saveDraft = vi.spyOn(mailClient, "saveDraft")
      .mockImplementationOnce(() => first.promise)
      .mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const onClose = vi.fn();
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={onClose} onQueued={() => {}} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Subject" }), { target: { value: "Keep me" } });
    fireEvent.click(screen.getByRole("button", { name: "Save and Close Draft" }));
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => { first.reject(new Error("Disk unavailable")); });
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toHaveTextContent("Not saved");
    expect(screen.getByRole("alert")).toHaveTextContent("Disk unavailable");
    const unload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(unload);
    expect(unload.defaultPrevented).toBe(true);

    const editor = screen.getByRole("textbox", { name: "Message Body" });
    editor.innerHTML = "Edits after failure";
    fireEvent.input(editor);
    fireEvent.click(screen.getByRole("button", { name: "Retry Save" }));
    await act(async () => {});
    expect(saveDraft.mock.calls[1][0]).toMatchObject({ revision: 0, subject: "Keep me", body: "Edits after failure" });
    expect(screen.queryByRole("alert")).toBeNull();
    const savedUnload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(savedUnload);
    expect(savedUnload.defaultPrevented).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Save and Close Draft" }));
    await act(async () => {});
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(saveDraft).toHaveBeenCalledTimes(2);
  });

  it("waits for the latest revision before sending once during an outstanding autosave", async () => {
    const first = deferred<Draft>();
    const second = deferred<Draft>();
    const saveDraft = vi.spyOn(mailClient, "saveDraft")
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise);
    const item: OutboxItem = { id: "queued-1", draft, state: "undo_pending", deadline: 10, error: null };
    const queueDraft = vi.spyOn(mailClient, "queueDraft").mockResolvedValue(item);
    const onQueued = vi.fn();
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={onQueued} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    editor.innerHTML = "Autosaved text";
    fireEvent.input(editor);
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    editor.innerHTML = "Final text";
    fireEvent.input(editor);
    act(() => { ref.current!.send(); ref.current!.send(); });
    expect(queueDraft).not.toHaveBeenCalled();
    await act(async () => { first.resolve({ ...saveDraft.mock.calls[0][0], revision: 1 }); });
    expect(saveDraft.mock.calls[1][0]).toMatchObject({ revision: 1, body: "Final text" });
    expect(queueDraft).not.toHaveBeenCalled();
    await act(async () => { second.resolve({ ...saveDraft.mock.calls[1][0], revision: 2 }); });
    expect(queueDraft).toHaveBeenCalledExactlyOnceWith("draft-1", 2);
    expect(onQueued).toHaveBeenCalledExactlyOnceWith(item);
  });

  it("saves during continuous input and cancels autosave timers on unmount", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: false });
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const { unmount } = render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    for (let index = 0; index < 15; index++) {
      editor.textContent = `Continuous input ${index}`;
      fireEvent.input(editor);
      await act(async () => { await vi.advanceTimersByTimeAsync(200); });
    }
    expect(saveDraft).toHaveBeenCalledTimes(1);
    expect(saveDraft.mock.calls[0][0].body).toBe("Continuous input 14");
    editor.textContent = "Unmounted change";
    fireEvent.input(editor);
    unmount();
    await vi.advanceTimersByTimeAsync(6_000);
    expect(saveDraft).toHaveBeenCalledTimes(1);
  });

  it("discards a completely empty draft when the composer closes", async () => {
    const onClose = vi.fn();
    const discardDraft = vi.spyOn(mailClient, "discardDraft").mockResolvedValue();
    const saveDraft = vi.spyOn(mailClient, "saveDraft");
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={onClose} onQueued={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: "Save and Close Draft" }));

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(discardDraft).toHaveBeenCalledWith("draft-1");
    expect(saveDraft).not.toHaveBeenCalled();
  });

  it("keeps a draft with authored content when the composer closes", async () => {
    const onClose = vi.fn();
    const discardDraft = vi.spyOn(mailClient, "discardDraft").mockResolvedValue();
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={onClose} onQueued={() => {}} />);

    fireEvent.change(screen.getByRole("textbox", { name: "Subject" }), { target: { value: "Keep this draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Save and Close Draft" }));

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({ subject: "Keep this draft" }));
    expect(discardDraft).not.toHaveBeenCalled();
  });
});

describe("Composer pasted images", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("inserts, resizes, and removes an image pasted into the message body", async () => {
    const inlineAttachment = { id: "inline-1", name: "screenshot.png", mime: "image/png", size: 4, ready: true, messageId: null, providerId: null, inline: true, contentId: "inline-1@threestrands.local" };
    const attachInline = vi.spyOn(mailClient, "attachInlineImage").mockResolvedValue({ ...draft, revision: 1, attachments: [inlineAttachment] });
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const removeAttachment = vi.spyOn(mailClient, "removeAttachment").mockResolvedValue({ ...draft, revision: 3 });
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    Object.defineProperty(editor, "clientWidth", { configurable: true, value: 800 });
    const imageFile = new File([new Uint8Array([137, 80, 78, 71])], "screenshot.png", { type: "image/png" });

    fireEvent.paste(editor, {
      clipboardData: { files: [imageFile], getData: () => "" },
    });

    const image = await waitFor(() => {
      const pasted = editor.querySelector<HTMLImageElement>('img[alt="screenshot.png"]');
      expect(pasted).toBeInTheDocument();
      return pasted!;
    });
    expect(attachInline).toHaveBeenCalledWith("draft-1", "screenshot.png", "image/png", expect.any(String));
    const handle = screen.getByRole("slider", { name: "Resize Pasted Image" });
    fireEvent.pointerDown(handle, { clientX: 100 });
    fireEvent.pointerMove(window, { clientX: 160 });
    fireEvent.pointerUp(window);
    expect(image).toHaveAttribute("width", "380");
    await waitFor(() => expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({
      bodyHtml: expect.stringContaining('src="cid:inline-1@threestrands.local"'),
    })));

    fireEvent.click(screen.getByRole("button", { name: "Remove Pasted Image" }));
    expect(editor.querySelector("img")).not.toBeInTheDocument();
    await waitFor(() => expect(removeAttachment).toHaveBeenCalledWith("draft-1", "inline-1"));
  });

  it("restores an inline image preview when a saved draft is reopened", async () => {
    const savedDraft = {
      ...draft,
      bodyHtml: '<p>See below</p><img src="cid:inline-1@threestrands.local" alt="Screenshot" width="320">',
      attachments: [{ id: "inline-1", name: "screenshot.png", mime: "image/png", size: 4, ready: true, messageId: null, providerId: null, inline: true, contentId: "inline-1@threestrands.local" }],
    };
    const readInline = vi.spyOn(mailClient, "readInlineImage").mockResolvedValue("data:image/png;base64,iVBORw==");

    render(<Composer draft={savedDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    const image = screen.getByRole("img", { name: "Screenshot" });
    await waitFor(() => expect(image).toHaveAttribute("src", "data:image/png;base64,iVBORw=="));
    expect(readInline).toHaveBeenCalledWith("draft-1", "inline-1");
    expect(screen.getByRole("button", { name: "Remove Pasted Image" })).toBeInTheDocument();
  });
});

describe("Composer forwarded content and attachments", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it.each([
    '<p><strong>Golf Monday</strong></p><a href="https://example.com/profile?one=1&amp;two=2">Profile</a><img src="https://images.example.com/forward-paragraph.png" alt="Invitation">',
    '<table cellspacing="4"><tr><td></td><td style="padding:8px"><em>Golf Monday</em><a href="https://example.com/profile?one=1&amp;two=2">Profile</a><img src="https://images.example.com/forward-table.png" alt="Invitation"></td></tr></table>',
  ])("preserves forwarded markup through editing and reopening in an isolated preview (%s)", async (html) => {
    const forwardedContent = { html: `<div>---------- Forwarded message ----------<br>From: Other &lt;other@example.com&gt;<br><br></div>${html}`, text: "Golf Monday [Profile] and [image]" };
    const forwarded: Draft = { ...draft, mode: "forward", subject: "Fwd: Golf Monday", forwardedContent };
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const proxy = vi.spyOn(mailClient, "fetchRemoteImage");
    const ref = createRef<ComposerHandle>();
    const { unmount } = render(<Composer ref={ref} draft={forwarded} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    expect(editor).toBeEmptyDOMElement();
    const frame = within(screen.getByRole("region", { name: "Forwarded message" })).getByLabelText("Message content") as HTMLIFrameElement;
    const preview = new DOMParser().parseFromString(frame.srcdoc, "text/html");
    expect(preview.querySelector("strong, em")?.textContent).toBe("Golf Monday");
    expect(preview.querySelector('a[href^="https:"]')?.getAttribute("href")).toBe("https://example.com/profile?one=1&two=2");
    expect(preview.querySelector("img")?.hasAttribute("src")).toBe(false);
    if (html.startsWith("<table")) {
      expect(preview.querySelector("table")?.getAttribute("cellspacing")).toBe("4");
      expect(preview.querySelectorAll("td")).toHaveLength(2);
      expect(preview.querySelector("td")?.innerHTML).toBe("");
    }
    expect(proxy).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Show quoted content" })).not.toBeInTheDocument();
    editor.innerHTML = "<b>See below.</b>";
    fireEvent.input(editor);
    let saved!: Draft;
    await act(async () => { saved = await ref.current!.flush(); });
    expect(saveDraft).toHaveBeenCalledWith(expect.objectContaining({ bodyHtml: "<b>See below.</b>", body: "See below.", forwardedContent }));
    unmount();
    render(<Composer draft={saved} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveTextContent("See below.");
    expect(screen.getByLabelText("Message content")).toHaveAttribute("srcdoc", frame.srcdoc);
  });

  it("keeps sender capabilities out of the editor and loads remote images only through the proxy", async () => {
    const url = "https://images.example.com/forward-security.png";
    const proxy = vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue("data:image/png;base64,cG5n");
    const forwardedContent = { html: `<p onclick="parent.compromised=true" style="position:fixed;background-image:url(${url})">Invitation</p><img src="${url}" onerror="parent.compromised=true" srcset="https://tracker.invalid/a 2x"><script>parent.compromised=true</script><iframe src="https://tracker.invalid"></iframe><a href="javascript:alert(1)">Unsafe</a>`, text: "Invitation" };
    render(<Composer draft={{ ...draft, mode: "forward", forwardedContent }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} messageAppearance={{ theme: "light" }} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    expect(editor).toBeEmptyDOMElement();
    const frame = screen.getByLabelText("Message content") as HTMLIFrameElement;
    const preview = new DOMParser().parseFromString(frame.srcdoc, "text/html");
    expect(frame).toHaveAttribute("sandbox", "allow-same-origin allow-scripts");
    expect(frame).toHaveAttribute("referrerpolicy", "no-referrer");
    expect(frame.srcdoc).toContain("script-src 'none'");
    expect(frame.srcdoc).toContain("img-src data:");
    expect(preview.documentElement.getAttribute("data-theme")).toBe("light");
    expect(preview.querySelector("script, iframe, [onclick], [onerror], [srcset]")).toBeNull();
    expect(preview.querySelector("a")?.hasAttribute("href")).toBe(false);
    expect(preview.querySelector("img")?.hasAttribute("src")).toBe(false);
    expect(preview.querySelector("p")!.style.position).toBe("");
    expect(preview.querySelector("p")!.style.backgroundImage).toBe("");
    expect(proxy).not.toHaveBeenCalled();
    frame.contentDocument!.body.innerHTML = preview.body.innerHTML;
    fireEvent.load(frame);
    fireEvent.click(screen.getByRole("button", { name: "Load images" }));
    await waitFor(() => expect(proxy).toHaveBeenCalledWith(url));
    await waitFor(() => expect(frame.contentDocument!.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,cG5n"));
  });

  it("retries embedded image previews after the forwarded attachment finishes downloading", async () => {
    const attachment = { id: "embedded-forward", name: "photo.png", mime: "image/png", size: 3, ready: false, messageId: "source", providerId: "part", inline: true, contentId: "photo@example.com" };
    const forwarded: Draft = { ...draft, mode: "forward", attachments: [attachment], forwardedContent: { html: '<p>Photos</p><img src="cid:photo%40example.com" alt="Photo">', text: "Photos" } };
    const ready = { ...forwarded, revision: 1, attachments: [{ ...attachment, ready: true }] };
    const downloaded = deferred<Draft>();
    vi.spyOn(mailClient, "fetchAttachment").mockReturnValue(downloaded.promise);
    const readImage = vi.spyOn(mailClient, "readInlineImage")
      .mockRejectedValueOnce(new Error("Inline image is unavailable"))
      .mockResolvedValue("data:image/png;base64,cG5n");
    const proxy = vi.spyOn(mailClient, "fetchRemoteImage");
    render(<Composer draft={forwarded} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const frame = screen.getByLabelText("Message content") as HTMLIFrameElement;
    frame.contentDocument!.body.innerHTML = new DOMParser().parseFromString(frame.srcdoc, "text/html").body.innerHTML;
    fireEvent.load(frame);
    await waitFor(() => expect(readImage).toHaveBeenCalledTimes(1));
    expect(frame.contentDocument!.querySelector("img")).not.toHaveAttribute("src");
    await act(async () => { downloaded.resolve(ready); });
    await waitFor(() => expect(frame.contentDocument!.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,cG5n"));
    expect(readImage).toHaveBeenCalledWith("draft-1", "embedded-forward");
    expect(proxy).not.toHaveBeenCalled();
  });

  // A forward and an Edit as New copy both carry the source message's files.
  it.each(["forward", "new"] as const)("downloads unresolved source attachments automatically before sending a %s draft", async (mode) => {
    const image = {
      id: "forwarded-image",
      name: "image003.jpg",
      mime: "image/jpeg",
      size: 4096,
      ready: false,
      messageId: "source-message",
      providerId: "provider-image",
      inline: false,
      contentId: null,
    };
    const forwarded = {
      ...draft,
      mode,
      to: "friend@example.com",
      attachments: [image],
    };
    const ready = {
      ...forwarded,
      revision: 1,
      attachments: [{ ...image, ready: true }],
    };
    const fetchAttachment = vi.spyOn(mailClient, "fetchAttachment").mockResolvedValue(ready);
    const queued = {
      id: "queued-forward",
      draft: ready,
      state: "undo_pending" as const,
      deadline: Date.now() + 10_000,
      error: null,
    };
    const queueDraft = vi.spyOn(mailClient, "queueDraft").mockResolvedValue(queued);
    const onQueued = vi.fn();

    render(<Composer draft={forwarded} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={onQueued} />);

    await waitFor(() => expect(fetchAttachment).toHaveBeenCalledWith("draft-1", "forwarded-image"));
    await waitFor(() => expect(screen.getByText(/Ready/)).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(queueDraft).toHaveBeenCalledWith("draft-1", 1));
    expect(onQueued).toHaveBeenCalledWith(queued);
  });
});

describe("Composer Reply Assist", () => {
  const replyDraft: Draft = {
    ...draft,
    mode: "reply",
    sourceId: "source-message",
    threadId: "provider-thread",
    to: "sender@example.com",
    subject: "Project timing",
    body: "\n\nOn Sep 16, Sender wrote:\n> Can we meet Friday?",
  };
  const context = {
    subject: "Project timing",
    messages: [{
      sender: "Sender <sender@example.com>",
      sentAt: "2026-09-16T12:00:00Z",
      bodyText: "Can we meet Friday?",
    }],
  };

  beforeEach(async () => {
    saveAiProvider("openai");
    saveAiFeatures({ ...DEFAULT_AI_FEATURES, draftAssist: true });
    await setAiApiKey("test-key");
  });

  afterEach(async () => {
    cleanup();
    vi.restoreAllMocks();
    await clearAiApiKey();
    saveAiProvider("none");
    saveAiFeatures(DEFAULT_AI_FEATURES);
  });

  it.each(["reply", "replyAll"] as const)("groups writing tools with the quote toggle below the %s body", async (mode) => {
    render(<Composer draft={{ ...replyDraft, mode }} accounts={accounts} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} review={{ enabled: true, available: true }} />);
    const tools = await screen.findByRole("group", { name: "Writing tools" });
    const trigger = await within(tools).findByRole("button", { name: "Draft reply" });
    const review = within(tools).getByRole("button", { name: "Review" });
    const toggle = screen.getByRole("button", { name: "Show Quoted Text" });
    expect(toggle.parentElement).toBe(tools.parentElement);
    expect(screen.getByRole("textbox", { name: "Message Body" }).compareDocumentPosition(toggle) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(tools.closest(".composer-content")).not.toBeNull();
    const quote = screen.getByLabelText("Quoted Text");
    expect(quote).not.toBeVisible();
    fireEvent.click(toggle);
    expect(quote).toBeVisible();
    expect(screen.getByRole("button", { name: "Hide Quoted Text" }).parentElement).toBe(tools.parentElement);
    expect(trigger).toBeEnabled();
    fireEvent.click(review);
    expect(screen.getByRole("region", { name: "Draft Review" })).toBeVisible();
    expect(quote).toBeVisible();
  });

  it("offers Draft reply once Draft Assist is on, and explains a missing key instead of doing nothing", async () => {
    await clearAiApiKey();
    const replyAssistContext = vi.spyOn(mailClient, "replyAssistContext");
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const trigger = await screen.findByRole("button", { name: /Draft reply/ });
    expect(trigger.querySelector("svg.lucide-sparkles")).not.toBeNull();
    expect(trigger).toHaveAttribute("title", "Draft a reply with AI (Ctrl J)");
    expect(trigger.querySelector("kbd")).toBeNull();
    expect(within(screen.getByRole("group", { name: "Writing tools" })).getByRole("button", { name: "Draft reply" })).toBe(trigger);

    act(() => ref.current?.draftReplyWithAI());
    const assist = await screen.findByRole("dialog", { name: "Reply Assist" });
    expect(within(assist).getByRole("alert")).toHaveTextContent("Set up an AI provider and API key in AI settings to draft replies with AI.");
    const generate = within(assist).getByRole("button", { name: "Generate Draft" });
    expect(generate).toBeDisabled();
    expect(generate.querySelector("svg.lucide-sparkles")).not.toBeNull();
    expect(replyAssistContext).not.toHaveBeenCalled();
  });

  it("leaves Draft reply out when no AI provider is chosen", async () => {
    saveAiProvider("none");
    render(<Composer draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    await screen.findByRole("textbox", { name: "Message Body" });
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByRole("button", { name: /Draft reply/ })).not.toBeInTheDocument();
  });

  it("shows the exact reviewed context and inserts provider output only as plain text", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const generate = vi.spyOn(mailClient, "generateReply").mockResolvedValue({
      body: '<img src=x onerror="alert(1)">Friday works for me.',
    });
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));

    render(<Composer draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: /Draft reply/ }));

    expect(await screen.findByText("Can we meet Friday?")).toBeInTheDocument();
    expect(screen.getByText("Project timing")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Optional Short Instruction" }), {
      target: { value: "Accept and ask what time." },
    });
    fireEvent.click(screen.getByRole("button", { name: "Generate Draft" }));

    await waitFor(() => expect(generate).toHaveBeenCalledWith(
      context,
      "Accept and ask what time.",
      "openai",
      "gpt-4o",
      null,
      "default",
    ));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Reply Assist" })).not.toBeInTheDocument());
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    await waitFor(() => expect(editor).toHaveTextContent('<img src=x onerror="alert(1)">Friday works for me.'));
    expect(editor.querySelector("img")).toBeNull();
    expect(screen.getByLabelText("Quoted Text")).toHaveTextContent("Can we meet Friday?");
  });

  it("opens a reply with the quoted source as a citation and saves a quoted text alternative", async () => {
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    const editor = screen.getByRole("textbox", { name: "Message Body" });
    const quoted = screen.getByLabelText("Quoted Text");
    const citation = quoted.querySelector('blockquote[type="cite"]');
    expect(citation).toHaveTextContent("Can we meet Friday?");
    expect(citation?.textContent).not.toContain(">");

    editor.insertBefore(document.createTextNode("Friday works."), editor.firstChild);
    fireEvent.input(editor);

    await waitFor(() => expect(saveDraft).toHaveBeenCalled());
    const saved = saveDraft.mock.calls.at(-1)![0];
    expect(saved.bodyHtml).toContain('<blockquote type="cite"');
    expect(saved.body).toBe("Friday works.\n\nOn Sep 16, Sender wrote:\n> Can we meet Friday?");
  });

  it("inserts selected availability text above the editable quoted reply", async () => {
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(
      <Composer
        draft={replyDraft}
        accounts={accounts} {...snippetProps}
        onClose={() => {}}
        onQueued={() => {}}
        availabilityText={'Here are some times that work for me:\n\n- Tuesday, September 22, 2026 · 9:00 AM–9:30 AM EDT'}
      />,
    );

    const editor = screen.getByRole("textbox", { name: "Message Body" });
    await waitFor(() => expect(editor).toHaveTextContent("September 22, 2026"));
    expect(editor).not.toHaveTextContent("Can we meet Friday?");
    expect(screen.getByLabelText("Quoted Text")).toHaveTextContent("Can we meet Friday?");
  });

  it("shows a task-derived instruction in Reply Assist without generating automatically", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const generate = vi.spyOn(mailClient, "generateReply");
    render(
      <Composer
        draft={replyDraft}
        accounts={accounts} {...snippetProps}
        onClose={() => {}}
        onQueued={() => {}}
        replyAssistInstruction="Draft a concise follow-up about the launch date."
      />,
    );

    const assist = await screen.findByRole("dialog", { name: "Reply Assist" });
    expect(within(assist).getByText(/Task-derived instruction:/)).toBeInTheDocument();
    expect(generate).not.toHaveBeenCalled();
  });

  it("preserves the editable draft when Reply Assist fails", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    vi.spyOn(mailClient, "generateReply").mockRejectedValue(new Error("Provider unavailable"));
    render(
      <Composer
        draft={replyDraft}
        accounts={accounts} {...snippetProps}
        onClose={() => {}}
        onQueued={() => {}}
        replyAssistInstruction="Draft a concise follow-up."
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Generate Draft" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Provider unavailable");
    expect(screen.getByLabelText("Quoted Text")).toHaveTextContent("Can we meet Friday?");
  });

  it("requires confirmation before adding a suggestion above existing authored text", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const generate = vi.spyOn(mailClient, "generateReply").mockResolvedValue({ body: "Suggested reply." });
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const existing = { ...replyDraft, body: `My existing words.${replyDraft.body}` };

    render(<Composer draft={existing} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: /Draft reply/ }));
    await screen.findByText("Can we meet Friday?");
    fireEvent.click(screen.getByRole("button", { name: "Generate Draft" }));

    expect(generate).not.toHaveBeenCalled();
    expect(screen.getByText(/already contains text/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add Anyway" }));

    await waitFor(() => expect(generate).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Reply Assist" })).not.toBeInTheDocument());
    const editor = screen.getByRole("textbox", { name: "Message Body" });
    await waitFor(() => expect(editor).toHaveTextContent("Suggested reply."));
    expect(editor).toHaveTextContent("My existing words.");
  });

  it("opens Reply Assist through the exposed handle, the same way the Mod+J shortcut does", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const ref = createRef<ComposerHandle>();

    render(<Composer ref={ref} draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    await screen.findByRole("button", { name: /Draft reply/ });

    ref.current?.draftReplyWithAI();

    expect(await screen.findByText("Can we meet Friday?")).toBeInTheDocument();
  });

  it("opens over the composer, focuses the instruction, and restores focus when dismissed", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const { container } = render(<Composer draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const trigger = await screen.findByRole("button", { name: /Draft reply/ });
    trigger.focus();

    fireEvent.click(trigger);

    const assist = await screen.findByRole("dialog", { name: "Reply Assist" });
    expect(assist).toHaveAttribute("aria-modal", "true");
    expect(assist).toHaveClass("reply-assist-modal");
    expect(assist.parentElement).toHaveClass("reply-assist-backdrop");
    expect(assist.parentElement?.parentElement).toBe(document.body);
    expect(container).toHaveAttribute("aria-hidden", "true");
    expect(screen.getByRole("textbox", { name: "Optional Short Instruction" })).toHaveFocus();

    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.queryByRole("dialog", { name: "Reply Assist" })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Reply Message" })).toBeInTheDocument();
    expect(trigger).toHaveFocus();
    expect(container).not.toHaveAttribute("aria-hidden");
  });

  it("keeps a dismissed task-opened assist closed until the user opens it again", async () => {
    vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} replyAssistInstruction="Ask for available times." />);

    const assist = await screen.findByRole("dialog", { name: "Reply Assist" });
    fireEvent.click(within(assist).getByRole("button", { name: "Close" }));

    expect(screen.queryByRole("dialog", { name: "Reply Assist" })).not.toBeInTheDocument();
    ref.current?.draftReplyWithAI();
    expect(await screen.findByRole("dialog", { name: "Reply Assist" })).toBeInTheDocument();
  });

  it("does nothing when Reply Assist is unavailable", async () => {
    saveAiProvider("none");
    const replyAssistContext = vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const ref = createRef<ComposerHandle>();

    render(<Composer ref={ref} draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    ref.current?.draftReplyWithAI();

    expect(screen.queryByText("Reply Assist")).not.toBeInTheDocument();
    expect(replyAssistContext).not.toHaveBeenCalled();
  });

  it("does not reopen or reload Reply Assist when it is already open", async () => {
    const replyAssistContext = vi.spyOn(mailClient, "replyAssistContext").mockResolvedValue(context);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={replyDraft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    await screen.findByRole("button", { name: /Draft reply/ });

    act(() => ref.current?.draftReplyWithAI());
    const assist = await screen.findByRole("dialog", { name: "Reply Assist" });
    expect(await within(assist).findByText("Can we meet Friday?")).toBeInTheDocument();
    fireEvent.change(within(assist).getByRole("textbox", { name: "Optional Short Instruction" }), { target: { value: "Keep it short" } });

    act(() => ref.current?.draftReplyWithAI());

    expect(screen.getAllByRole("dialog", { name: "Reply Assist" })).toHaveLength(1);
    expect(within(assist).getByText("Can we meet Friday?")).toBeInTheDocument();
    expect(within(assist).getByRole("textbox", { name: "Optional Short Instruction" })).toHaveValue("Keep it short");
    expect(replyAssistContext).toHaveBeenCalledTimes(1);
  });
});

describe("Composer quoted history", () => {
  const reply: Draft = {
    ...draft,
    mode: "reply",
    sourceId: "source-message",
    to: "sender@example.com",
    subject: "Project timing",
    body: "\n\nOn Sep 16, Sender wrote:\n> Can we meet Friday?",
  };

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("keeps a reply's quoted history out of the body editor, collapsed until shown", () => {
    render(<Composer draft={reply} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const body = screen.getByRole("textbox", { name: "Message Body" });
    const quoted = screen.getByLabelText("Quoted Text");
    const toggle = screen.getByRole("button", { name: "Show Quoted Text" });

    expect(body).toBeEmptyDOMElement();
    expect(body).toHaveFocus();
    expect(quoted).not.toBeVisible();
    expect(quoted.querySelector('blockquote[type="cite"]')).toHaveTextContent("Can we meet Friday?");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveAttribute("aria-controls", quoted.id);

    fireEvent.click(toggle);
    expect(quoted).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Quoted Text" })).toBe(quoted);
    expect(screen.getByRole("button", { name: "Hide Quoted Text" })).toHaveAttribute("aria-expanded", "true");

    fireEvent.click(screen.getByRole("button", { name: "Hide Quoted Text" }));
    expect(quoted).not.toBeVisible();
  });

  it("saves edits to the shown quoted history after the authored text", async () => {
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    render(<Composer draft={reply} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const body = screen.getByRole("textbox", { name: "Message Body" });
    fireEvent.click(screen.getByRole("button", { name: "Show Quoted Text" }));
    const quoted = screen.getByRole("textbox", { name: "Quoted Text" });

    body.textContent = "Friday works.";
    fireEvent.input(body);
    quoted.querySelector("blockquote")!.textContent = "Can we meet Saturday?";
    fireEvent.input(quoted);

    await waitFor(() => expect(saveDraft).toHaveBeenCalled());
    const saved = saveDraft.mock.calls.at(-1)![0];
    expect(saved.body).toBe("Friday works.\n\nOn Sep 16, Sender wrote:\n> Can we meet Saturday?");
    expect(saved.bodyHtml).toMatch(/^Friday works\.<br><br>On Sep 16, Sender wrote:<blockquote type="cite"/);
  });

  it("shows an explicitly selected quote immediately and keeps focus in the reply body", () => {
    render(<Composer draft={reply} accounts={accounts} {...snippetProps} expandQuotedText onClose={() => {}} onQueued={() => {}} />);
    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveFocus();
    expect(screen.getByRole("textbox", { name: "Quoted Text" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Hide Quoted Text" })).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(screen.getByRole("button", { name: "Hide Quoted Text" }));
    expect(screen.getByLabelText("Quoted Text")).not.toBeVisible();
  });

  it("splits a reopened reply draft's saved HTML back into authored text and collapsed history", async () => {
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const reopened: Draft = {
      ...reply,
      body: "Friday works.\n\nOn Sep 16, Sender wrote:\n> Can we meet Friday?",
      bodyHtml: 'Friday <b>works</b>.<br><br>On Sep 16, Sender wrote:<blockquote type="cite">Can we meet Friday?</blockquote>',
    };
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={reopened} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveTextContent(/^Friday works\.$/);
    expect(screen.getByLabelText("Quoted Text")).not.toBeVisible();

    fireEvent.input(screen.getByRole("textbox", { name: "Message Body" }));
    await ref.current?.flush();
    expect(saveDraft.mock.calls.at(-1)![0]).toEqual(expect.objectContaining({ body: reopened.body }));
    expect(saveDraft.mock.calls.at(-1)![0].bodyHtml).toMatch(/^Friday <b>works<\/b>\.<br><br>On Sep 16, Sender wrote:<blockquote type="cite"/);
  });

  it("leaves new messages and inline answers in a single body editor", () => {
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.queryByRole("button", { name: "Show Quoted Text" })).toBeNull();
    cleanup();

    const inline: Draft = { ...reply, bodyHtml: 'On Sep 16, Sender wrote:<blockquote type="cite">Can we meet Friday?</blockquote>Yes, Friday.' };
    render(<Composer draft={inline} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.queryByRole("button", { name: "Show Quoted Text" })).toBeNull();
    expect(screen.getByRole("textbox", { name: "Message Body" })).toHaveTextContent("Can we meet Friday?");
  });
});

describe("Composer recipient autocomplete", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  const contact = {
    email: "jane@example.com",
    displayName: "Jane Doe",
    sentCount: 4,
    receivedCount: 1,
    lastInteractedAt: "2026-03-05T00:00:00Z",
    pinned: false,
  };

  it("ignores outdated autocomplete responses during debounce and after clearing", async () => {
    const old = deferred<typeof contact[]>();
    const current = deferred<typeof contact[]>();
    const suggest = vi.spyOn(mailClient, "listContactSuggestions")
      .mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });
    fireEvent.change(to, { target: { value: "ja" } });
    await act(async () => { await vi.advanceTimersByTimeAsync(150); });
    fireEvent.change(to, { target: { value: "jo" } });
    await act(async () => { old.resolve([contact]); });
    expect(screen.queryByRole("option", { name: /Jane Doe/ })).not.toBeInTheDocument();
    await act(async () => { await vi.advanceTimersByTimeAsync(150); });
    expect(suggest).toHaveBeenCalledTimes(2);
    fireEvent.change(to, { target: { value: "" } });
    await act(async () => { current.resolve([contact]); });
    expect(screen.queryByRole("option", { name: /Jane Doe/ })).not.toBeInTheDocument();
  });

  it("loads contact groups once while a composer remains open", async () => {
    const groups = vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([]);
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });
    await act(async () => { to.focus(); });
    const subject = screen.getByRole("textbox", { name: "Subject" });
    await act(async () => { subject.focus(); to.focus(); });
    expect(groups).toHaveBeenCalledTimes(1);
  });

  it("suggests a past correspondent from local history and fills the field on selection", async () => {
    const suggest = vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([contact]);
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });

    fireEvent.change(to, { target: { value: "ja" } });
    await vi.advanceTimersByTimeAsync(150);
    expect(suggest).toHaveBeenCalledWith("first@example.com", "ja", 8);

    const option = await screen.findByRole("option", { name: /Jane Doe/ });
    fireEvent.mouseDown(option);
    expect(screen.getByRole("button", { name: "Remove Jane Doe" })).toBeInTheDocument();
    expect(to).toHaveValue("");
  });

  const board = {
    id: "group-board",
    name: "Board",
    members: [
      { contactId: "contact:ada", displayName: "Ada Park", email: "ada@home.example", addresses: ["ada@home.example", "ada@work.example"] },
      { contactId: "contact:jane", displayName: "Jane Doe", email: "jane@example.com", addresses: ["jane@example.com"] },
      { contactId: "contact:sam", displayName: null, email: "sam@example.com", addresses: ["sam@example.com"] },
    ],
  };

  it("offers a matching group first and adds each member's primary address as a chip", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([contact]);
    vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([board, { id: "empty", name: "Boardgames", members: [] }]);
    const onDraftChange = vi.fn();
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    render(<Composer draft={{ ...draft, to: "Jane Doe <jane@example.com>, " }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} onDraftChange={onDraftChange} />);
    const to = screen.getByRole("textbox", { name: "To" });
    fireEvent.focus(to);
    fireEvent.change(to, { target: { value: "boa" } });
    await vi.advanceTimersByTimeAsync(150);

    const options = within(await screen.findByRole("listbox", { name: "To suggestions" })).getAllByRole("option");
    // A group with no members is not offered.
    expect(options[0]).toHaveAccessibleName("Board, group of 3 members");
    expect(screen.queryByRole("option", { name: /Boardgames/ })).not.toBeInTheDocument();
    expect(to).toHaveAttribute("aria-activedescendant", "to-group-0");
    fireEvent.keyDown(to, { key: "Enter" });

    expect(screen.getByRole("button", { name: "Remove Ada Park" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove sam@example.com" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Remove Jane Doe" })).toHaveLength(1);
    expect(screen.getByText(/^Added 2 people/)).toHaveTextContent("Added 2 people from Board · 1 already in To");
    expect(screen.getByText(/^Added 2 people/)).toHaveAttribute("role", "status");
    // The note sits under the field's row, not inside it, so it can't push the chips off the label's line.
    expect(screen.getByText(/^Added 2 people/).closest(".recipient-field")).toBeNull();
    expect(to.closest(".recipient-field")?.nextElementSibling).toBe(screen.getByText(/^Added 2 people/));
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ to: "Jane Doe <jane@example.com>, Ada Park <ada@home.example>, sam@example.com, " }));
    fireEvent.change(to, { target: { value: "x" } });
    expect(screen.queryByText(/^Added 2 people/)).not.toBeInTheDocument();
  });

  it("quotes a group member whose name holds a comma, so the draft saves as one recipient", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([]);
    vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([{ id: "eo", name: "EO", members: [
      { contactId: "contact:justin", displayName: "Fischgrund, Justin", email: "justin@example.com", addresses: ["justin@example.com"] },
      { contactId: "contact:kelly", displayName: "Kelly Sjol", email: "kelly@example.com", addresses: ["kelly@example.com"] },
    ] }]);
    const save = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });
    fireEvent.focus(to);
    fireEvent.change(to, { target: { value: "eo" } });
    await vi.advanceTimersByTimeAsync(150);
    fireEvent.mouseDown(await screen.findByRole("option", { name: /EO, group/ }));
    expect(screen.getByRole("button", { name: "Remove Fischgrund, Justin" })).toBeInTheDocument();
    await act(async () => { await ref.current!.flush(); });
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ to: '"Fischgrund, Justin" <justin@example.com>, Kelly Sjol <kelly@example.com>, ' }));
  });

  it("reads an older draft's unquoted comma as one recipient and saves it quoted", async () => {
    const save = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={{ ...draft, to: "Fischgrund, Justin <justin@example.com>, Kelly Sjol <kelly@example.com>, " }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    expect(screen.getByRole("button", { name: "Remove Fischgrund, Justin" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove Fischgrund" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Subject" }), { target: { value: "EO All Chapter Event" } });
    await act(async () => { await ref.current!.flush(); });
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ to: '"Fischgrund, Justin" <justin@example.com>, Kelly Sjol <kelly@example.com>, ' }));
  });

  it("moves through group and contact suggestions with the arrow keys", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([{ ...contact, displayName: "Board Chair", email: "chair@example.com" }]);
    vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([board]);
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });
    fireEvent.focus(to);
    fireEvent.change(to, { target: { value: "board" } });
    await vi.advanceTimersByTimeAsync(150);
    await screen.findByRole("option", { name: /Board Chair/ });
    fireEvent.keyDown(to, { key: "ArrowDown" });
    expect(to).toHaveAttribute("aria-activedescendant", "to-suggestion-0");
    fireEvent.keyDown(to, { key: "Enter" });
    expect(screen.getByRole("button", { name: "Remove Board Chair" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove Ada Park" })).not.toBeInTheDocument();
  });

  it("pins a suggested contact without inserting it into the field", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([contact]);
    const pin = vi.spyOn(mailClient, "pinContact").mockResolvedValue();
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });

    fireEvent.change(to, { target: { value: "ja" } });
    await vi.advanceTimersByTimeAsync(150);
    await screen.findByRole("option", { name: /Jane Doe/ });

    fireEvent.click(screen.getByRole("button", { name: "Pin jane@example.com" }));
    expect(pin).toHaveBeenCalledWith("first@example.com", "jane@example.com", "Jane Doe");
    expect(to).toHaveValue("ja");
  });

  it("offers to pin a brand-new address that has no mail history at all", async () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([]);
    const pin = vi.spyOn(mailClient, "pinContact").mockResolvedValue();
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });

    fireEvent.change(to, { target: { value: "wife@example.com" } });
    await vi.advanceTimersByTimeAsync(150);

    const option = await screen.findByRole("option", { name: /Pin wife@example.com as a contact/ });
    fireEvent.mouseDown(option);

    expect(pin).toHaveBeenCalledWith("first@example.com", "wife@example.com", null);
    expect(screen.getByRole("button", { name: "Remove wife@example.com" })).toBeInTheDocument();
    expect(to).toHaveValue("");
  });

  it("shows a prefilled reply recipient as a badge immediately, with no mail history query needed", () => {
    vi.spyOn(mailClient, "listContactSuggestions").mockResolvedValue([]);
    const prefilled = { ...draft, to: "hello@threestrands.local" };
    render(<Composer draft={prefilled} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    expect(screen.getByRole("button", { name: "Remove hello@threestrands.local" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "To" })).toHaveValue("");
  });

  it("removes a recipient badge via its remove button and via Backspace on an empty field", () => {
    const prefilled = { ...draft, to: "a@example.com, b@example.com" };
    render(<Composer draft={prefilled} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: "Remove a@example.com" }));
    expect(screen.queryByRole("button", { name: "Remove a@example.com" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove b@example.com" })).toBeInTheDocument();

    fireEvent.keyDown(screen.getByRole("textbox", { name: "To" }), { key: "Backspace" });
    expect(screen.queryByRole("button", { name: "Remove b@example.com" })).not.toBeInTheDocument();
  });

  it("removes a newly added recipient without activating the first recipient's remove button", () => {
    const prefilled = { ...draft, to: "original@example.com" };
    render(<Composer draft={prefilled} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const to = screen.getByRole("textbox", { name: "To" });

    fireEvent.change(to, { target: { value: "added@example.com" } });
    fireEvent.keyDown(to, { key: "Enter" });

    // The chip buttons must not be descendants of the input's label. Native
    // label activation would otherwise forward a click to the first button.
    expect(to.closest("label")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Remove added@example.com" }));

    expect(screen.getByRole("button", { name: "Remove original@example.com" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove added@example.com" })).not.toBeInTheDocument();
  });

  it("drags a recipient badge from To into Cc, moving it rather than copying it", () => {
    const prefilled = { ...draft, to: "hello@threestrands.local", cc: "" };
    render(<Composer draft={prefilled} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "To" }));

    const chip = screen.getByRole("button", { name: "Remove hello@threestrands.local" }).closest(".recipient-chip");
    const ccRow = screen.getByRole("textbox", { name: "Cc" }).closest(".recipient-chip-row");
    expect(chip).toBeTruthy();
    expect(ccRow).toBeTruthy();

    const store = new Map<string, string>();
    const dataTransfer = {
      setData: (type: string, val: string) => store.set(type, val),
      getData: (type: string) => store.get(type) ?? "",
      dropEffect: "move",
      effectAllowed: "move",
    };

    fireEvent.dragStart(chip!, { dataTransfer });
    fireEvent.dragOver(ccRow!, { dataTransfer });
    fireEvent.drop(ccRow!, { dataTransfer });
    fireEvent.dragEnd(chip!, { dataTransfer });

    const toField = screen.getByRole("textbox", { name: "To" }).closest(".compose-field") as HTMLElement;
    const ccField = screen.getByRole("textbox", { name: "Cc" }).closest(".compose-field") as HTMLElement;
    expect(within(toField).queryByRole("button", { name: "Remove hello@threestrands.local" })).not.toBeInTheDocument();
    expect(within(ccField).getByRole("button", { name: "Remove hello@threestrands.local" })).toBeInTheDocument();
  });
});

describe("Composer context panel actions", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it.each([false, true])("inserts and saves a snippet at the selection after its picker takes focus (quoted=%s)", async (quoted) => {
    const saveDraft = vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const ref = createRef<ComposerHandle>();
    const initial: Draft = { ...draft, mode: "reply", to: "Ann <ann@example.com>", bodyHtml: 'Hello world<br><br>On Monday, Sender wrote:<blockquote type="cite">Quoted words</blockquote>' };
    render(<Composer ref={ref} draft={initial} accounts={accounts} {...snippetProps} snippets={[{ id: "greeting", name: "Greeting", body: "<b>{first_name}</b>", createdAt: "2026-01-01" }]} onClose={() => {}} onQueued={() => {}} />);
    if (quoted) fireEvent.click(screen.getByRole("button", { name: "Show Quoted Text" }));
    const editor = screen.getByRole("textbox", { name: quoted ? "Quoted Text" : "Message Body" });
    editor.focus();
    const text = quoted ? editor.querySelector("blockquote")!.firstChild! : editor.firstChild!;
    const range = document.createRange();
    range.setStart(text, 0);
    range.setEnd(text, quoted ? 6 : 5);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    fireEvent.keyDown(editor, { key: ";", ctrlKey: true });
    expect(screen.getByRole("combobox", { name: "Find or Create a Snippet" })).toHaveFocus();
    fireEvent.click(screen.getByRole("option", { name: /Greeting/ }));
    expect(screen.queryByRole("dialog", { name: "Insert Snippet" })).toBeNull();
    expect(editor).toHaveFocus();
    expect(editor.querySelector("b")).toHaveTextContent("Ann");
    expect(editor).toHaveTextContent(quoted ? "Ann words" : "Ann world");
    await act(async () => { await ref.current!.flush(); });
    const savedBody = document.createElement("div");
    savedBody.innerHTML = saveDraft.mock.calls.at(-1)![0].bodyHtml!;
    const quote = savedBody.querySelector('blockquote[type="cite"]')!;
    expect(quote.innerHTML).toBe(quoted ? "<b>Ann</b> words" : "Quoted words");
    quote.remove();
    expect(savedBody.innerHTML).toBe(`${quoted ? "Hello world" : "<b>Ann</b> world"}<br><br>On Monday, Sender wrote:`);
  });

  it("reports recipient edits as they happen and body text at autosave", async () => {
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const onDraftChange = vi.fn();
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={draft} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} onDraftChange={onDraftChange} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Subject" }), { target: { value: "Lunch" } });
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ subject: "Lunch" }));

    const editor = screen.getByRole("textbox", { name: "Message Body" });
    editor.innerHTML = "See you there";
    fireEvent.input(editor);
    await waitFor(() => expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ body: "See you there" })));
  });

  it("moves the named addresses from To and Cc to Bcc", async () => {
    const onDraftChange = vi.fn();
    const ref = createRef<ComposerHandle>();
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    render(<Composer ref={ref} draft={{ ...draft, to: "Ann <ann@example.com>, bob@example.com, ", cc: "cy@example.com", bcc: "" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} onDraftChange={onDraftChange} />);
    act(() => ref.current!.moveRecipientsToBcc(["ANN@example.com", "cy@example.com"]));
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ to: "bob@example.com", cc: "", bcc: "Ann <ann@example.com>, cy@example.com" }));
    expect(screen.getByRole("textbox", { name: "Bcc" })).toBeInTheDocument();
  });

  it("swaps one recipient for another and leaves the rest", async () => {
    const onDraftChange = vi.fn();
    const ref = createRef<ComposerHandle>();
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    render(<Composer ref={ref} draft={{ ...draft, to: "Ann <ann@example.com>, jonh@example.com, " }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} onDraftChange={onDraftChange} />);
    act(() => ref.current!.replaceRecipient("JONH@example.com", "John <john@example.com>"));
    expect(onDraftChange).toHaveBeenLastCalledWith(expect.objectContaining({ to: "Ann <ann@example.com>, John <john@example.com>" }));
    expect(screen.getByText("John")).toBeInTheDocument();
  });

  it("returns focus and inserts text where the caret was before the body lost focus, else at the top", () => {
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => next);
    const ref = createRef<ComposerHandle>();
    render(<Composer ref={ref} draft={{ ...draft, body: "Hi Ann,\nBest, Me" }} accounts={accounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const editor = screen.getByRole("textbox", { name: "Message Body" });

    act(() => ref.current!.insertText("Top line"));
    expect(editor.textContent?.startsWith("Top line")).toBe(true);

    // Put the caret after "Hi Ann," and leave the body.
    const walker = document.createTreeWalker(editor, NodeFilter.SHOW_TEXT);
    let greeting: Node | null = walker.nextNode();
    while (greeting && !greeting.textContent?.includes("Hi Ann,")) greeting = walker.nextNode();
    const caret = document.createRange();
    caret.setStart(greeting!, greeting!.textContent!.indexOf("Hi Ann,") + "Hi Ann,".length);
    caret.collapse(true);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(caret);
    fireEvent.blur(editor);

    act(() => ref.current!.focusBody());
    expect(editor).toHaveFocus();
    expect(window.getSelection()!.getRangeAt(0).startContainer).toBe(greeting);
    expect(window.getSelection()!.getRangeAt(0).startOffset).toBe(greeting!.textContent!.indexOf("Hi Ann,") + "Hi Ann,".length);

    act(() => ref.current!.insertText("Here are some times"));
    const text = editor.textContent ?? "";
    expect(text.indexOf("Hi Ann,")).toBeLessThan(text.indexOf("Here are some times"));
    expect(text.indexOf("Here are some times")).toBeLessThan(text.indexOf("Best, Me"));
  });
});


describe("Composer send options", () => {
  afterEach(() => {cleanup();vi.restoreAllMocks();});
  it("keeps secondary delivery actions in a keyboard-accessible menu and restores focus on Escape", () => {
    const onClose = vi.fn();
    const onSendAndArchive = vi.fn();
    render(<Composer draft={{ ...draft, mode: "reply" }} accounts={accounts} {...snippetProps} onClose={onClose} onQueued={vi.fn()} onSendAndArchive={onSendAndArchive} />);
    const trigger = screen.getByRole("button", { name: "Send options" });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Send later" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send" })).toHaveTextContent("Send Ctrl Enter");
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const later = screen.getByRole("menuitem", { name: "Send later…" });
    const archive = screen.getByRole("menuitem", { name: "Send & archive" });
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(later).toHaveFocus();
    expect(later).toHaveTextContent("Ctrl Shift L");
    expect(archive).toHaveTextContent("Ctrl Shift Enter");
    fireEvent.keyDown(later, { key: "ArrowDown" });
    expect(archive).toHaveFocus();
    fireEvent.keyDown(archive, { key: "Home" });
    expect(later).toHaveFocus();
    fireEvent.keyDown(later, { key: "ArrowUp" });
    expect(archive).toHaveFocus();
    fireEvent.keyDown(archive, { key: "Escape" });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("menuitem", { name: "Send & archive" }));
    expect(onSendAndArchive).toHaveBeenCalledOnce();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("offers only scheduling when archive is unavailable and dismisses on outside clicks or focus leaving", () => {
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: "Send options" });
    fireEvent.click(trigger);
    expect(screen.getAllByRole("menuitem")).toHaveLength(1);
    fireEvent.pointerDown(screen.getByRole("textbox", { name: "Subject" }));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    fireEvent.click(trigger);
    act(() => { screen.getByRole("textbox", { name: "Message Body" }).focus(); });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    fireEvent.click(trigger);
    fireEvent.keyDown(screen.getByRole("menuitem"), { key: "Tab" });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it.each([false, true])("honors shifted send from the menu with metaKey=%s without activating scheduling", async (metaKey) => {
    const archive = vi.fn();
    const schedule = vi.spyOn(mailClient, "schedulingInfo");
    render(<Composer draft={{ ...draft, mode: "reply" }} accounts={accounts} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} onSendAndArchive={archive} />);
    fireEvent.click(screen.getByRole("button", { name: "Send options" }));
    fireEvent.keyDown(screen.getByRole("menuitem", { name: "Send later…" }), { key: "Enter", metaKey, ctrlKey: !metaKey, shiftKey: true });
    expect(archive).toHaveBeenCalledOnce();
    expect(schedule).not.toHaveBeenCalled();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("sends normally from a focused menu item without opening scheduling", async () => {
    const queue = vi.spyOn(mailClient, "queueDraft").mockResolvedValue({ id: "queued", draft, state: "undo_pending", deadline: 10000, error: null });
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Send options" }));
    fireEvent.keyDown(screen.getByRole("menuitem", { name: "Send later…" }), { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(queue).toHaveBeenCalledWith("draft-1", 0));
    expect(screen.queryByRole("dialog", { name: "Send later" })).not.toBeInTheDocument();
  });

  it("disables sending and its options while the draft is being saved for delivery", async () => {
    const pending = deferred<Draft>();
    vi.spyOn(mailClient, "saveDraft").mockImplementation(() => pending.promise);
    vi.spyOn(mailClient, "queueDraft").mockResolvedValue({ id: "queued", draft, state: "undo_pending", deadline: 10000, error: null });
    render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={vi.fn()} onQueued={vi.fn()} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Subject" }), { target: { value: "Sending" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send options" })).toBeDisabled();
    await act(async () => { pending.resolve({ ...draft, subject: "Sending", revision: 1 }); });
  });

  it("flushes edited content before scheduling and closes only after durable queueing",async () => {
    vi.spyOn(mailClient,"schedulingInfo").mockResolvedValue({deviceName:"MacBook",sharing:true});
    vi.spyOn(mailClient,"scheduleChoices").mockResolvedValue([{scheduledAt:Date.now()+3600000,offsetSeconds:0}]);
    const save=vi.spyOn(mailClient,"saveDraft").mockImplementation(async (next) => ({...next,revision:next.revision+1}));
    const queued:OutboxItem={id:"scheduled",draft,state:"scheduled",deadline:Date.now()+3600000,error:null};
    const schedule=vi.spyOn(mailClient,"scheduleDraft").mockResolvedValue(queued);
    const immediate=vi.spyOn(mailClient,"queueDraft");const onQueued=vi.fn(),onClose=vi.fn();
    render(<Composer draft={{...draft,to:"client@example.com"}} accounts={accounts} {...snippetProps} onClose={onClose} onQueued={onQueued}/>);
    fireEvent.change(screen.getByRole("textbox",{name:"Subject"}),{target:{value:"Saved before scheduling"}});
    fireEvent.click(screen.getByRole("button", { name: "Send options" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Send later…" }));
    const dialog=screen.getByRole("dialog",{name:"Send later"});
    await waitFor(() => expect(within(dialog).getByRole("button",{name:"Schedule email"})).toBeEnabled());
    expect(within(dialog).getByText(/Recipients, message bodies, and attachments stay/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button",{name:"Schedule email"}));
    await waitFor(() => expect(onQueued).toHaveBeenCalledWith(queued));
    expect(save.mock.calls.at(-1)?.[0].subject).toBe("Saved before scheduling");expect(schedule.mock.calls[0][1]).toBe(1);expect(immediate).not.toHaveBeenCalled();expect(onClose).not.toHaveBeenCalled();
  });
  it("requires an explicit offset for repeated times and Escape keeps the composer open",async () => {
    vi.spyOn(mailClient,"schedulingInfo").mockResolvedValue({deviceName:"Desktop",sharing:false});
    vi.spyOn(mailClient,"scheduleChoices").mockResolvedValue([{scheduledAt:Date.now()+3600000,offsetSeconds:-14400},{scheduledAt:Date.now()+7200000,offsetSeconds:-18000}]);
    const close=vi.fn();render(<Composer draft={draft} accounts={accounts} {...snippetProps} onClose={close} onQueued={vi.fn()}/>);
    fireEvent.click(screen.getByRole("button", { name: "Send options" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Send later…" }));
    await waitFor(() => expect(screen.getByRole("combobox",{name:"UTC Offset"})).toBeInTheDocument());
    expect(screen.getByRole("button",{name:"Schedule email"})).toBeDisabled();
    fireEvent.change(screen.getByRole("combobox",{name:"UTC Offset"}),{target:{value:"-18000"}});
    expect(screen.getByRole("button",{name:"Schedule email"})).toBeEnabled();
    fireEvent.keyDown(window,{key:"Escape"});
    await waitFor(() => expect(screen.queryByRole("dialog",{name:"Send later"})).not.toBeInTheDocument());expect(close).not.toHaveBeenCalled();expect(screen.getByRole("dialog",{name:"New Message"})).toBeInTheDocument();
  });
});

describe("Composer sending-address picker", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  const imapAccounts: Account[] = ["me@proton.me"].map((email) => ({
    email,
    displayName: null,
    color: "#6D4AFF",
    status: "connected",
    provider: "imap",
    labelModel: "imapLabelFolders",
    sortOrder: 0,
    connectedAt: "2026-01-01T00:00:00Z",
    lastSyncedAt: null,
    onThisDevice: true,
  }));
  const imapDraft: Draft = { ...draft, account: "me@proton.me" };
  const twoIdentities = [
    { address: "me@proton.me" },
    { address: "work@proton.me", displayName: "Work" },
  ];

  it("hides the picker when the account has a single identity", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: "me@proton.me" }]);
    render(<Composer draft={imapDraft} accounts={imapAccounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    await waitFor(() => expect(mailClient.listIdentities).toHaveBeenCalledWith("me@proton.me"));
    expect(screen.queryByRole("combobox", { name: "Sending Address" })).not.toBeInTheDocument();
  });

  it.each(["new", "reply"] as const)("shows the picker with several identities (%s mode) and sets from", async (mode) => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue(twoIdentities);
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const setDraftFrom = vi.spyOn(mailClient, "setDraftFrom").mockResolvedValue({ ...imapDraft, mode, revision: 5, from: "work@proton.me" });
    render(<Composer draft={{ ...imapDraft, mode }} accounts={imapAccounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const picker = await screen.findByRole("combobox", { name: "Sending Address" });
    expect(within(picker).getByRole("option", { name: "Work <work@proton.me>" })).toBeInTheDocument();
    await act(async () => { fireEvent.change(picker, { target: { value: "work@proton.me" } }); });
    await waitFor(() => expect(setDraftFrom).toHaveBeenCalledWith("draft-1", "work@proton.me"));
  });

  it("stores null when the primary identity is chosen", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue(twoIdentities);
    vi.spyOn(mailClient, "saveDraft").mockImplementation(async (next) => ({ ...next, revision: next.revision + 1 }));
    const setDraftFrom = vi.spyOn(mailClient, "setDraftFrom").mockResolvedValue({ ...imapDraft, revision: 5, from: null });
    render(<Composer draft={{ ...imapDraft, from: "work@proton.me" }} accounts={imapAccounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const picker = await screen.findByRole("combobox", { name: "Sending Address" });
    await act(async () => { fireEvent.change(picker, { target: { value: "me@proton.me" } }); });
    await waitFor(() => expect(setDraftFrom).toHaveBeenCalledWith("draft-1", null));
  });

  it("shows the draft's chosen from in the picker value", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue(twoIdentities);
    render(<Composer draft={{ ...imapDraft, from: "work@proton.me" }} accounts={imapAccounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    const picker = await screen.findByRole("combobox", { name: "Sending Address" }) as HTMLSelectElement;
    expect(picker.value).toBe("work@proton.me");
  });

  it("hides the picker when fetching identities fails and never blocks sending", async () => {
    vi.spyOn(mailClient, "listIdentities").mockRejectedValue(new Error("offline"));
    render(<Composer draft={imapDraft} accounts={imapAccounts} {...snippetProps} onClose={() => {}} onQueued={() => {}} />);
    await waitFor(() => expect(mailClient.listIdentities).toHaveBeenCalled());
    expect(screen.queryByRole("combobox", { name: "Sending Address" })).not.toBeInTheDocument();
    // Send is still available (not disabled by the failed fetch).
    expect(screen.getByRole("button", { name: "Send" })).toBeEnabled();
  });
});
