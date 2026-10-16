import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { SignaturesSettings } from "./SignaturesSettings";
import { SignaturePreview } from "./SignaturePreview";
import { SignatureEditor, SignatureImportDialog } from "./SignatureEditor";
import { mailClient } from "./data/client";
import { readSignatureEditor, signatureDefaultsKey, type Signature, type SignatureState } from "./signatures";
import type { Account } from "./domain";
import DOMPurify from "dompurify";
import * as signatureImport from "./signatureImport";
import { loadSignatureEditorImages } from "./signatureEditorImages";
import { EMAIL_IMAGE_LIMITS, SIGNATURE_LIMITS } from "./emailRenderingPolicy";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const signature: Signature = { id: "work", name: "Work", html: "<p>Alex</p>", text: "Alex" };
const account = { email: "me@example.com", nickname: "Work" } as Account;
function library(overrides: Partial<SignatureState> = {}): SignatureState {
  return { signatures: [signature], defaults: [], save: vi.fn().mockResolvedValue(undefined), remove: vi.fn().mockResolvedValue(undefined), saveDefaults: vi.fn().mockResolvedValue(undefined), inheritDefaults: vi.fn().mockResolvedValue(undefined), refresh: vi.fn(), ...overrides };
}

describe("signature settings", () => {
  it("keeps settings cards read-only and opens an explicit modal editor with focus and cancellation", async () => {
    const state = library();
    const { container } = render(<SignaturesSettings library={state} accounts={[account]} />);
    expect(container.querySelector("input,select,textarea,[contenteditable]")).toBeNull();
    const add = screen.getByRole("button", { name: "Add Signature" }); add.focus(); fireEvent.click(add);
    const dialog = screen.getByRole("dialog", { name: "New Signature" });
    expect(dialog).toHaveAttribute("data-shortcut-scope", "modal");
    expect(within(dialog).getByLabelText("Name")).toHaveFocus();
    expect(container).toHaveAttribute("aria-hidden", "true");
    expect((container as HTMLElement).inert).toBe(true);
    const save = within(dialog).getByRole("button", { name: "Save" }); save.focus();
    fireEvent.keyDown(save, { key: "Tab" });
    expect(within(dialog).getByRole("button", { name: "Close" })).toHaveFocus();
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(add).toHaveFocus(); expect(state.save).not.toHaveBeenCalled();
    expect((container as HTMLElement).inert).toBeFalsy();
  });

  it("duplicates only on Save and preserves edits when saving fails", async () => {
    const state = library({ save: vi.fn().mockRejectedValue(new Error("Storage unavailable")) });
    render(<SignaturesSettings library={state} accounts={[account]} />);
    fireEvent.click(screen.getByRole("button", { name: "Duplicate" }));
    const dialog = screen.getByRole("dialog", { name: "Edit Signature" });
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Work Copy");
    expect(state.save).not.toHaveBeenCalled();
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Short" } });
    const body = within(dialog).getByRole("textbox", { name: "Signature Content" }); body.innerHTML = "<b>Thanks</b>"; fireEvent.input(body);
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Storage unavailable");
    expect(body.innerHTML).toBe("<b>Thanks</b>");
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Short");
    expect(state.save).toHaveBeenCalledWith(expect.objectContaining({ name: "Short", html: "<b>Thanks</b>", text: "Thanks" }));
    expect(vi.mocked(state.save).mock.calls[0][0].id).not.toBe(signature.id);
  });

  it("preserves independently edited plain text through rich edits, failures and reopening", async () => {
    const save = vi.fn().mockRejectedValueOnce(new Error("Storage unavailable")).mockResolvedValue(undefined);
    const source = { ...signature, text: "Custom alternative" };
    const view = render(<SignatureEditor signature={source} onSave={save} onClose={vi.fn()} />);
    const plain = screen.getByLabelText("Plain Text");
    expect(plain).toHaveValue("Custom alternative");
    const body = screen.getByRole("textbox", { name: "Signature Content" });
    body.innerHTML = "<p>Updated visual</p>"; fireEvent.input(body);
    expect(plain).toHaveValue("Custom alternative");
    fireEvent.change(plain, { target: { value: "Corrected alternative" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Storage unavailable");
    expect(plain).toHaveValue("Corrected alternative");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
    expect(save.mock.calls[1][0]).toEqual(expect.objectContaining({ html: "<p>Updated visual</p>", text: "Corrected alternative" }));
    view.unmount();
    render(<SignatureEditor signature={save.mock.calls[1][0]} onSave={save} onClose={vi.fn()} />);
    expect(screen.getByLabelText("Plain Text")).toHaveValue("Corrected alternative");
    fireEvent.click(screen.getByRole("button", { name: "Regenerate from Signature" }));
    expect(screen.getByLabelText("Plain Text")).toHaveValue("Updated visual");
    expect(screen.getByRole("textbox", { name: "Signature Content" }).innerHTML).toBe("<p>Updated visual</p>");
  });

  it.each([
    '<table><tr><td><img src="https://images.example/logo.png" alt="Logo" width="32"></td><td>Alex</td><td></td></tr></table>',
    '<div style="display:flex"><img src="https://images.example/logo.png" alt="Logo" width="32"><p>Alex</p></div>',
  ])("loads hosted logos for editing without embedding the preview data on save: %s", async (html) => {
    const png = "data:image/png;base64,aA==";
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockRejectedValueOnce(new Error("Image source is blocked")).mockResolvedValue(png);
    const save = vi.fn();
    render(<SignatureEditor signature={{ ...signature, html }} onSave={save} onClose={vi.fn()} />);
    const content = screen.getByRole("textbox", { name: "Signature Content" });
    const image = content.querySelector("img")!;
    expect(image).not.toHaveAttribute("src"); expect(fetch).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Load Images" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Image source is blocked");
    expect(image).not.toHaveAttribute("src"); expect(save).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Load Images" }));
    await waitFor(() => expect(image).toHaveAttribute("src", png));
    expect(image).toHaveAttribute("data-compose-source", "https://images.example/logo.png");
    fireEvent.click(screen.getByRole("button", { name: "Load Images" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Images loaded for editing"));
    expect(fetch).toHaveBeenCalledTimes(2);
    fireEvent.click(image);
    const dialog = screen.getByRole("dialog", { name: "Edit Signature Image" });
    expect(within(dialog).getByRole("img")).toHaveAttribute("src", png);
    fireEvent.change(within(dialog).getByLabelText("Width in Pixels"), { target: { value: "48" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply" }));
    expect(image).toHaveAttribute("src", png); expect(image).toHaveAttribute("width", "48");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0][0].html).toContain('src="https://images.example/logo.png"');
    expect(save.mock.calls[0][0].html).not.toMatch(/data:image|data-compose/);
  });

  it.each(["data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=", "https://evil.example/image.png", "javascript:alert(1)"])("rejects unsafe editor proxy results: %s", async (data) => {
    vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue(data);
    render(<SignatureEditor signature={{ ...signature, html: '<p>Alex<img src="https://images.example/logo.png"></p>' }} onSave={vi.fn()} onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Load Images" }));
    expect(await screen.findByRole("status")).toHaveTextContent("did not return a supported image");
    expect(screen.getByRole("textbox", { name: "Signature Content" }).querySelector("img")).not.toHaveAttribute("src");
  });

  it("does not apply an image response to an editor image replaced while loading", async () => {
    let finish!: (data: string) => void;
    vi.spyOn(mailClient, "fetchRemoteImage").mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    render(<SignatureEditor signature={{ ...signature, html: '<img src="https://images.example/logo.png">' }} onSave={vi.fn()} onClose={vi.fn()} />);
    const root = screen.getByRole("textbox", { name: "Signature Content" });
    fireEvent.click(screen.getByRole("button", { name: "Load Images" }));
    await waitFor(() => expect(finish).toBeDefined());
    root.innerHTML = '<img data-compose-source="https://images.example/replacement.png">';
    await act(async () => { finish("data:image/png;base64,aA=="); });
    expect(root.querySelector("img")).not.toHaveAttribute("src");
    expect(root.querySelector("img")).toHaveAttribute("data-compose-source", "https://images.example/replacement.png");
  });

  it.each([EMAIL_IMAGE_LIMITS.maxImagesPerMessage - 1, EMAIL_IMAGE_LIMITS.maxImagesPerMessage, EMAIL_IMAGE_LIMITS.maxImagesPerMessage + 1])("uses the shared editor preview image count boundary at %s", async (count) => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue("data:image/png;base64,aA==");
    const root = document.createElement("div"); document.body.append(root);
    root.innerHTML = Array.from({ length: count }, (_, index) => `<img data-compose-source="https://images.example/${index}.png">`).join("");
    try {
      const failures = await loadSignatureEditorImages(root);
      expect(fetch).toHaveBeenCalledTimes(Math.min(count, EMAIL_IMAGE_LIMITS.maxImagesPerMessage));
      expect(root.querySelectorAll("img[src]")).toHaveLength(Math.min(count, EMAIL_IMAGE_LIMITS.maxImagesPerMessage));
      expect(failures).toHaveLength(count > EMAIL_IMAGE_LIMITS.maxImagesPerMessage ? 1 : 0);
    } finally { root.remove(); }
  });

  it.each([-1, 0, 1])("uses the shared editor image byte boundary at offset %s", async (offset) => {
    const limit = EMAIL_IMAGE_LIMITS.maxDataUriBytesPerMessage;
    const data = "data:image/png;base64,";
    vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue(data + "A".repeat(limit + offset - data.length));
    const root = document.createElement("div"); document.body.append(root);
    root.innerHTML = '<img data-compose-source="https://images.example/large.png">';
    try {
      const failures = await loadSignatureEditorImages(root);
      expect(root.querySelector("img")!.hasAttribute("src")).toBe(offset <= 0);
      expect(failures).toHaveLength(offset <= 0 ? 0 : 1);
    } finally { root.remove(); }
  });

  it.each(["embedded", "hosted"])("counts existing %s images toward the aggregate editor budget", async (kind) => {
    const data = "data:image/png;base64,";
    const large = data + "A".repeat(EMAIL_IMAGE_LIMITS.maxDataUriBytesPerMessage - data.length);
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue("data:image/png;base64,aA==");
    const root = document.createElement("div"); document.body.append(root);
    root.innerHTML = '<img><img data-compose-source="https://images.example/other.png">';
    root.querySelector("img")!.src = large;
    if (kind === "hosted") root.querySelector("img")!.dataset.composeSource = "https://images.example/already-loaded.png";
    try {
      const failures = await loadSignatureEditorImages(root);
      expect(failures[0]).toContain("too large to preview together");
      expect(root.querySelectorAll("img")[1]).not.toHaveAttribute("src");
      expect(fetch).toHaveBeenCalledTimes(1);
    } finally { root.remove(); }
  });

  it("requires an explicit deletion and preserves the library on failure", async () => {
    const state = library({ remove: vi.fn().mockRejectedValue(new Error("Could not delete")) });
    render(<SignaturesSettings library={state} accounts={[]} />);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(state.remove).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog", { name: "Delete Signature" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Could not delete");
    expect(state.remove).toHaveBeenCalledWith("work");
  });

  it("distinguishes alias None from account inheritance and saves only after submission", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: account.email }, { address: "alias@example.com" }]);
    const base = { id: signatureDefaultsKey(account.email), account: account.email, address: "", newSignatureId: signature.id, replySignatureId: signature.id };
    const alias = { ...base, id: signatureDefaultsKey(account.email, "alias@example.com"), address: "alias@example.com", newSignatureId: null, replySignatureId: null };
    const state = library({ defaults: [base, alias] });
    render(<SignaturesSettings library={state} accounts={[account]} />);
    const trigger = screen.getByRole("button", { name: /Edit Signature Defaults/ }); trigger.focus(); fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Default Signatures" });
    expect(within(dialog).getByLabelText("Sending Address")).toHaveFocus();
    await within(dialog).findByRole("option", { name: "alias@example.com" });
    fireEvent.change(within(dialog).getByLabelText("Sending Address"), { target: { value: "alias@example.com" } });
    expect(within(dialog).getByLabelText("New Messages")).toHaveValue("");
    expect(within(dialog).getByLabelText("Use Account Defaults")).not.toBeChecked();
    fireEvent.click(within(dialog).getByLabelText("Use Account Defaults"));
    expect(within(dialog).getByLabelText("New Messages")).toHaveValue("work");
    expect(within(dialog).getByLabelText("New Messages")).toBeDisabled();
    expect(state.inheritDefaults).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(state.inheritDefaults).toHaveBeenCalledWith(account.email, "alias@example.com"));
    expect(state.saveDefaults).not.toHaveBeenCalled();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("saves independent new and reply defaults and retains values on failure", async () => {
    vi.spyOn(mailClient, "listIdentities").mockResolvedValue([{ address: account.email }]);
    const state = library({ saveDefaults: vi.fn().mockRejectedValue(new Error("Offline storage failure")) });
    render(<SignaturesSettings library={state} accounts={[account]} />);
    fireEvent.click(screen.getByRole("button", { name: /Edit Signature Defaults/ }));
    const dialog = screen.getByRole("dialog", { name: "Default Signatures" });
    fireEvent.change(within(dialog).getByLabelText("New Messages"), { target: { value: "work" } });
    expect(state.saveDefaults).not.toHaveBeenCalled();
    fireEvent.keyDown(within(dialog).getByLabelText("New Messages"), { key: "Enter", ctrlKey: true });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Offline storage failure");
    expect(within(dialog).getByLabelText("New Messages")).toHaveValue("work");
    expect(state.saveDefaults).toHaveBeenCalledWith(expect.objectContaining({ newSignatureId: "work", replySignatureId: null }));
  });
});

describe("signature import", () => {
  it.each([SIGNATURE_LIMITS.maxImageBytes - 1, SIGNATURE_LIMITS.maxImageBytes, SIGNATURE_LIMITS.maxImageBytes + 1])("bounds uploaded images at %s bytes before persistence", async (size) => {
    const save = vi.fn();
    render(<SignatureEditor signature={null} onSave={save} onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Add Image…" }));
    const dialog = screen.getByRole("dialog", { name: "Add Signature Image" });
    expect(within(dialog).getByLabelText("Image")).toHaveFocus();
    fireEvent.change(within(dialog).getByLabelText("Image"), { target: { files: [new File([new Uint8Array(size)], "logo.png", { type: "image/png" })] } });
    if (size <= SIGNATURE_LIMITS.maxImageBytes) await waitFor(() => expect(dialog.querySelector("img")).not.toBeNull());
    else expect(await within(dialog).findByRole("alert")).toHaveTextContent("too large");
    expect(save).not.toHaveBeenCalled();
  });
  it("uses the sandboxed preview, keeps hosted images by default and sanitizes imported content", async () => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage"); const onImport = vi.fn();
    render(<SignatureImportDialog onClose={vi.fn()} onImport={onImport} />);
    const html = screen.getByLabelText("HTML"); expect(html).toHaveFocus();
    fireEvent.change(html, { target: { value: '<table><tr><td>Alex</td><td></td></tr></table><img src="https://images.example/logo.png"><script>alert(1)</script>' } });
    const preview = screen.getByRole("region", { name: "Signature Preview" });
    expect(preview.querySelector("iframe")).toHaveAttribute("sandbox", "allow-same-origin allow-scripts");
    expect(preview.querySelector("iframe")?.srcdoc).toContain("script-src 'none'");
    expect(screen.getByLabelText("Embed Remote Images")).not.toBeChecked();
    expect(fetch).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(onImport).toHaveBeenCalled());
    expect(onImport.mock.calls[0][0]).toContain("<td></td>");
    expect(onImport.mock.calls[0][0]).not.toMatch(/script/);
    expect(onImport.mock.calls[0][0]).toContain('src="https://images.example/logo.png"');
  });

  it("downloads opted-in images through the native proxy and keeps HTML when a source is blocked", async () => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockRejectedValue(new Error("Image source is blocked"));
    const onImport = vi.fn();
    render(<SignatureImportDialog onClose={vi.fn()} onImport={onImport} />);
    const source = '<p>Alex</p><img src="https://images.example/logo.png" alt="Logo">';
    fireEvent.change(screen.getByLabelText("HTML"), { target: { value: source } });
    expect(screen.getByLabelText("Embed Remote Images")).not.toBeChecked();
    fireEvent.click(screen.getByLabelText("Embed Remote Images"));
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Image source is blocked");
    expect(screen.getByLabelText("HTML")).toHaveValue(source); expect(onImport).not.toHaveBeenCalled();
    fetch.mockResolvedValue("data:image/png;base64,aA==");
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(onImport).toHaveBeenCalledWith('<p>Alex</p><img src="data:image/png;base64,aA==" alt="Logo">'));
    expect(fetch).toHaveBeenCalledWith("https://images.example/logo.png");
  });

  /** Pastes HTML into a new signature's editor, as copying from a webmail signature page would. */
  function pasteSignature(html: string) {
    render(<SignatureEditor signature={null} onSave={vi.fn()} onClose={vi.fn()} />);
    const editor = screen.getByRole("textbox", { name: "Signature Content" });
    fireEvent.paste(editor, { clipboardData: { files: [], getData: (type: string) => type === "text/html" ? html : "" } });
    return editor;
  }
  /** A PNG data URI whose decoded payload is exactly `bytes` long. */
  const pngOfBytes = (bytes: number) => `data:image/png;base64,${Buffer.alloc(bytes).toString("base64")}`;

  it("keeps pasted hosted images inert in the editor and retains their sources on save", () => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage");
    const editor = pasteSignature('<table><tr><td><img src="https://images.example/logo.png" alt="Logo" width="80"></td><td>Alex</td></tr></table><div style="background-image:url(https://images.example/bg.png)">x</div>');
    expect(editor.querySelector("img")).not.toHaveAttribute("src");
    expect(editor.querySelector("img")).toHaveAttribute("data-compose-source", "https://images.example/logo.png");
    expect(editor.querySelector("img")).toHaveAttribute("alt", "Logo");
    expect(editor.querySelector("img")).toHaveAttribute("width", "80");
    expect(readSignatureEditor(editor).html).toContain('src="https://images.example/logo.png"');
    expect(editor.innerHTML).not.toContain("background-image");
    expect(fetch).not.toHaveBeenCalled();
    cleanup();
    const flow = pasteSignature('<p><img src="https://cdn.example/a.gif"> Alex Doe</p><p><a href="https://example.com"><img src="https://cdn.example/a.gif"></a><script>alert(1)</script></p>');
    expect(flow.querySelectorAll("img")).toHaveLength(2);
    expect(flow.querySelectorAll("img[src]")).toHaveLength(0);
    expect(readSignatureEditor(flow).html).toContain('src="https://cdn.example/a.gif"');
    expect(fetch).not.toHaveBeenCalled();
  });

  it("keeps every pasted hosted source without downloading or applying the embedding size limit", () => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockRejectedValue(new Error("Image source is blocked"));
    const editor = pasteSignature('<p>Alex</p><img src="https://images.example/blocked.png"><img src="https://images.example/large.png">');
    expect(editor.querySelectorAll("img")).toHaveLength(2);
    expect(readSignatureEditor(editor).html).toContain('src="https://images.example/large.png"');
    expect(screen.queryByRole("alert")).toBeNull();
    expect(fetch).not.toHaveBeenCalled();
  });

  it.each([SIGNATURE_LIMITS.maxImageBytes - 1, SIGNATURE_LIMITS.maxImageBytes, SIGNATURE_LIMITS.maxImageBytes + 1])("bounds explicitly embedded remote images at %s bytes like uploads", async (size) => {
    vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue(pngOfBytes(size));
    const onImport = vi.fn();
    render(<SignatureImportDialog onClose={vi.fn()} onImport={onImport} />);
    fireEvent.change(screen.getByLabelText("HTML"), { target: { value: '<p>Alex<img src="https://images.example/logo.png"></p>' } });
    fireEvent.click(screen.getByLabelText("Embed Remote Images"));
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    if (size <= SIGNATURE_LIMITS.maxImageBytes) {
      await waitFor(() => expect(onImport).toHaveBeenCalled());
      expect(onImport.mock.calls[0][0]).toContain(pngOfBytes(size));
      expect(screen.queryByRole("alert")).toBeNull();
    } else {
      expect(await screen.findByRole("alert")).toHaveTextContent("larger than 128 KiB");
      expect(onImport).not.toHaveBeenCalled();
    }
  });

  it("pastes signatures without remote images immediately and never fetches", () => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage");
    const editor = pasteSignature('<p>Alex</p><img src="data:image/png;base64,aA=="><img src="cid:logo@example.com">');
    expect(editor.querySelectorAll("img")).toHaveLength(1);
    expect(screen.queryByRole("status")).toBeNull();
    expect(fetch).not.toHaveBeenCalled();
  });

  it("Escape closes only the nested import dialog and restores its trigger without saving", async () => {
    const onSave = vi.fn();
    function Harness() { const [open, setOpen] = useState(true); return open ? <SignatureEditor signature={null} onSave={onSave} onClose={() => setOpen(false)} /> : null; }
    render(<Harness />);
    const trigger = screen.getByRole("button", { name: "Import…" }); trigger.focus(); fireEvent.click(trigger);
    const nested = screen.getByRole("dialog", { name: "Import Signature" });
    expect(screen.queryByRole("dialog", { name: "New Signature" })).toBeNull();
    await act(async () => fireEvent.keyDown(document, { key: "Escape" }));
    expect(nested).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "New Signature" })).toBeInTheDocument();
    expect(trigger).toHaveFocus(); expect(onSave).not.toHaveBeenCalled();
  });
});

describe("Gmail signature import", () => {
  const gmail = { ...account, provider: "gmail", onThisDevice: true } as Account;
  const remote = '<table><tr><td><b>Alex</b></td><td><a href="https://example.com">Website</a></td></tr></table><img src="https://images.example/logo.png" alt="Logo"><script>alert(1)</script><span onclick="alert(1)" style="background-image:url(https://bad.example/x)">Team</span>';

  it("limits import to Gmail accounts on this device and keeps settings read-only", () => {
    const { container } = render(<SignaturesSettings library={library()} accounts={[{ ...gmail, provider: "imap" }, { ...gmail, email: "elsewhere@example.com", onThisDevice: false }]} />);
    expect(screen.getByRole("button", { name: "Import from Gmail…" })).toBeDisabled();
    expect(container.querySelector("input,select,textarea,[contenteditable]")).toBeNull();
  });

  it("opens a keyboard-scoped review with intentional focus, containment, cancellation and restoration", async () => {
    const list = vi.spyOn(mailClient, "listGmailSignatures").mockResolvedValue([{ address: gmail.email, html: "<p>Alex</p>" }]);
    const save = vi.spyOn(mailClient, "importGmailSignature").mockResolvedValue(undefined);
    const { container } = render(<SignaturesSettings library={library()} accounts={[gmail]} />);
    const trigger = screen.getByRole("button", { name: "Import from Gmail…" }); trigger.focus(); fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Import from Gmail" });
    expect(dialog).toHaveAttribute("data-shortcut-scope", "modal");
    const input = within(dialog).getByLabelText("Gmail Account"); expect(input).toHaveFocus();
    expect((container as HTMLElement).inert).toBe(true);
    await within(dialog).findByLabelText("Name");
    const submit = within(dialog).getByRole("button", { name: "Import" }); submit.focus(); fireEvent.keyDown(submit, { key: "Tab" });
    const close = within(dialog).getByRole("button", { name: "Close" }); expect(close).toHaveFocus();
    fireEvent.keyDown(close, { key: "Tab", shiftKey: true }); expect(submit).toHaveFocus();
    expect(list).toHaveBeenCalledWith(gmail.email); expect(save).not.toHaveBeenCalled();
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(trigger).toHaveFocus()); expect((container as HTMLElement).inert).toBeFalsy();
    expect(save).not.toHaveBeenCalled();
  });

  it("previews sanitized content and saves a named copy only on explicit submission", async () => {
    vi.spyOn(mailClient, "listGmailSignatures").mockResolvedValue([{ address: gmail.email, html: remote }]);
    const save = vi.spyOn(mailClient, "importGmailSignature").mockResolvedValue(undefined);
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage");
    const state = library();
    render(<SignaturesSettings library={state} accounts={[gmail]} />);
    fireEvent.click(screen.getByRole("button", { name: "Import from Gmail…" }));
    const name = await screen.findByLabelText("Name");
    expect(screen.getByRole("region", { name: "Plain Text Preview" })).toHaveTextContent("Website (https://example.com)");
    expect(fetch).not.toHaveBeenCalled(); expect(save).not.toHaveBeenCalled();
    fireEvent.change(name, { target: { value: "Imported Work" } });
    fireEvent.click(screen.getByLabelText("Use as Account Default for New Messages"));
    fireEvent.keyDown(name, { key: "Enter", metaKey: true });
    await waitFor(() => expect(save).toHaveBeenCalled());
    const [email, signature, useDefault] = save.mock.calls[0];
    expect(email).toBe(gmail.email); expect(useDefault).toBe(true); expect(signature.name).toBe("Imported Work");
    expect(signature.html).toContain("<table>"); expect(signature.html).not.toMatch(/script|onclick|background-image/i);
    expect(signature.html).toContain('src="https://images.example/logo.png"');
    expect(signature.text).toContain("https://example.com");
    expect(state.refresh).toHaveBeenCalled(); expect(state.save).not.toHaveBeenCalled();
  });

  it("preserves the selected address, name and default choice when a save fails and retries with the same id", async () => {
    vi.spyOn(mailClient, "listGmailSignatures").mockResolvedValue([{ address: gmail.email, html: "<p>Primary</p>" }, { address: "alias@example.com", html: "<p>Alias</p>" }]);
    const save = vi.spyOn(mailClient, "importGmailSignature").mockRejectedValueOnce(new Error("Storage unavailable")).mockResolvedValue(undefined);
    render(<SignaturesSettings library={library()} accounts={[gmail]} />);
    fireEvent.click(screen.getByRole("button", { name: "Import from Gmail…" }));
    fireEvent.change(await screen.findByLabelText("Sending Address"), { target: { value: "alias@example.com" } });
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Alias Copy" } });
    fireEvent.click(screen.getByLabelText("Use as Account Default for New Messages"));
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Storage unavailable");
    expect(screen.getByLabelText("Sending Address")).toHaveValue("alias@example.com");
    expect(screen.getByLabelText("Name")).toHaveValue("Alias Copy");
    expect(screen.getByLabelText("Use as Account Default for New Messages")).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
    expect(save.mock.calls[1]).toEqual(save.mock.calls[0]);
    expect(save.mock.calls[0][1].text).toBe("Alias");
  });

  it("keeps embedding collapsed and saves corrected plain text without changing the visual signature", async () => {
    const html = '<p><img src="https://images.example/logo.png" alt="Alex">Alex</p>';
    vi.spyOn(mailClient, "listGmailSignatures").mockResolvedValue([{ address: gmail.email, html }, { address: "alias@example.com", html: "<p>Alias</p>" }]);
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue("data:image/png;base64,aA==");
    const save = vi.spyOn(mailClient, "importGmailSignature").mockRejectedValueOnce(new Error("Offline")).mockResolvedValue(undefined);
    render(<SignaturesSettings library={library()} accounts={[gmail]} />);
    fireEvent.click(screen.getByRole("button", { name: "Import from Gmail…" }));
    const plain = await screen.findByLabelText("Plain Text");
    expect(plain).toHaveValue("AlexAlex");
    const options = screen.getByText("Image Options").closest("details")!;
    expect(options).not.toHaveAttribute("open"); expect(fetch).not.toHaveBeenCalled();
    fireEvent.change(plain, { target: { value: "Alex" } });
    fireEvent.click(screen.getByText("Image Options"));
    fireEvent.click(screen.getByRole("button", { name: "Embed Remote Images" }));
    await screen.findByRole("status");
    expect(plain).toHaveValue("Alex");
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Offline");
    expect(plain).toHaveValue("Alex");
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
    expect(save.mock.calls[1][1]).toEqual(expect.objectContaining({ text: "Alex", html: '<p><img src="data:image/png;base64,aA==" alt="Alex">Alex</p>' }));
  });

  it("regenerates plain text explicitly and resets it when selecting another signature", async () => {
    vi.spyOn(mailClient, "listGmailSignatures").mockResolvedValue([{ address: gmail.email, html: "<p>Primary</p>" }, { address: "alias@example.com", html: "<p>Alias</p>" }]);
    render(<SignaturesSettings library={library()} accounts={[gmail]} />);
    fireEvent.click(screen.getByRole("button", { name: "Import from Gmail…" }));
    const plain = await screen.findByLabelText("Plain Text");
    fireEvent.change(plain, { target: { value: "Custom" } });
    fireEvent.click(screen.getByRole("button", { name: "Regenerate from Signature" }));
    expect(plain).toHaveValue("Primary");
    fireEvent.change(plain, { target: { value: "Custom" } });
    fireEvent.change(screen.getByLabelText("Sending Address"), { target: { value: "alias@example.com" } });
    expect(plain).toHaveValue("Alias");
  });

  it.each([
    '<table><tr><td><img src="https://images.example/logo.png" alt="Logo"></td><td>Alex</td></tr></table>',
    '<div style="display:flex"><img src="https://images.example/logo.png" alt="Logo"><p>Alex</p></div>',
  ])("does not reprocess signature HTML or reload the preview when editing plain text: %s", async (html) => {
    vi.spyOn(mailClient, "listGmailSignatures").mockResolvedValue([{ address: gmail.email, html }]);
    const convert = vi.spyOn(signatureImport, "signatureImportContent");
    const sanitize = vi.spyOn(DOMPurify, "sanitize");
    const save = vi.spyOn(mailClient, "importGmailSignature").mockResolvedValue(undefined);
    render(<SignaturesSettings library={library()} accounts={[gmail]} />);
    fireEvent.click(screen.getByRole("button", { name: "Import from Gmail…" }));
    const plain = await screen.findByLabelText("Plain Text");
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    const originalDoc = frame.srcdoc;
    const conversions = convert.mock.calls.length, sanitizations = sanitize.mock.calls.length;
    fireEvent.change(plain, { target: { value: "Alex " } });
    expect(plain).toHaveValue("Alex ");
    fireEvent.change(plain, { target: { value: "Alex Example" } });
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Work" } });
    expect(convert).toHaveBeenCalledTimes(conversions);
    expect(sanitize).toHaveBeenCalledTimes(sanitizations);
    expect(screen.getByTestId("message-body")).toBe(frame); expect(frame.srcdoc).toBe(originalDoc);
    expect(save).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0][1].text).toBe("Alex Example");
  });

  it("offers retry for provider failures and empty results without writing anything", async () => {
    const list = vi.spyOn(mailClient, "listGmailSignatures").mockRejectedValueOnce(new Error("Reconnect Gmail to continue")).mockResolvedValue([]);
    const save = vi.spyOn(mailClient, "importGmailSignature");
    render(<SignaturesSettings library={library()} accounts={[gmail]} />);
    fireEvent.click(screen.getByRole("button", { name: "Import from Gmail…" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Reconnect Gmail to continue");
    expect(screen.getByRole("button", { name: "Import" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));
    expect(await screen.findByText(/No signatures were found/)).toBeInTheDocument();
    expect(list).toHaveBeenCalledTimes(2); expect(save).not.toHaveBeenCalled();
  });

  it("embeds logos through the existing proxy before previewing and preserves the source on failure", async () => {
    vi.spyOn(mailClient, "listGmailSignatures").mockResolvedValue([{ address: gmail.email, html: remote }]);
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockRejectedValueOnce(new Error("Image source is blocked")).mockResolvedValue("data:image/png;base64,aA==");
    const save = vi.spyOn(mailClient, "importGmailSignature").mockResolvedValue(undefined);
    render(<SignaturesSettings library={library()} accounts={[gmail]} />);
    fireEvent.click(screen.getByRole("button", { name: "Import from Gmail…" }));
    fireEvent.click(await screen.findByRole("button", { name: "Embed Remote Images" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Image source is blocked");
    expect(save).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Embed Remote Images" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Embed Remote Images" })).toBeNull());
    expect(screen.getByRole("status")).toHaveTextContent("Images are embedded in the preview");
    const frame = screen.getByRole("region", { name: "Signature Preview" }).querySelector("iframe")!;
    frame.contentDocument!.body.innerHTML = new DOMParser().parseFromString(frame.srcdoc, "text/html").body.innerHTML;
    fireEvent.load(frame);
    await waitFor(() => expect(frame.contentDocument!.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,aA=="));
    expect(screen.getByRole("region", { name: "Plain Text Preview" })).toHaveTextContent("Logo");
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0][1].html).toContain('src="data:image/png;base64,aA=="');
    expect(fetch.mock.calls).toEqual([["https://images.example/logo.png"], ["https://images.example/logo.png"]]);
    expect(save.mock.calls[0][2]).toBe(false);
  });

  it("keeps a hosted image after an oversized embedding attempt and still allows importing its URL", async () => {
    const url = "https://images.example/large.png";
    vi.spyOn(mailClient, "listGmailSignatures").mockResolvedValue([{ address: gmail.email, html: `<p>Alex<img src="${url}" alt="Logo"></p>` }]);
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue(`data:image/png;base64,${Buffer.alloc(869 * 1024).toString("base64")}`);
    const save = vi.spyOn(mailClient, "importGmailSignature").mockResolvedValue(undefined);
    render(<SignaturesSettings library={library()} accounts={[gmail]} />);
    fireEvent.click(screen.getByRole("button", { name: "Import from Gmail…" }));
    fireEvent.click(await screen.findByRole("button", { name: "Embed Remote Images" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("larger than 128 KiB");
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0][1].html).toContain(`src="${url}"`);
    expect(save.mock.calls[0][1].html).not.toContain("data:image");
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("ignores an earlier account's response after switching accounts", async () => {
    let resolveFirst!: (value: { address: string; html: string }[]) => void;
    vi.spyOn(mailClient, "listGmailSignatures").mockImplementationOnce(() => new Promise((resolve) => { resolveFirst = resolve; }))
      .mockResolvedValue([{ address: "second@example.com", html: "<p>Second</p>" }]);
    render(<SignaturesSettings library={library()} accounts={[gmail, { ...gmail, email: "second@example.com" }]} />);
    fireEvent.click(screen.getByRole("button", { name: "Import from Gmail…" }));
    fireEvent.change(screen.getByLabelText("Gmail Account"), { target: { value: "second@example.com" } });
    expect(await screen.findByLabelText("Name")).toHaveValue("Gmail — second@example.com");
    await act(async () => resolveFirst([{ address: gmail.email, html: "<p>First</p>" }]));
    expect(screen.getByLabelText("Name")).toHaveValue("Gmail — second@example.com");
    expect(screen.getByRole("region", { name: "Plain Text Preview" })).toHaveTextContent("Second");
  });
});


describe("signature previews", () => {
  const png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j1ioAAAAASUVORK5CYII=";
  it.each([
    `<table><tr><td><img src="${png}" alt="Logo" width="32"></td><td>Alex</td><td></td></tr></table>`,
    `<div style="display:flex;gap:8px"><img src="${png}" alt="Logo" width="32"><p>Alex<br>Designer</p></div>`,
  ])("displays embedded images locally in an isolated preview for %s", async (html) => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage");
    render(<SignaturePreview html={html} text="Alex" />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    frame.contentDocument!.body.innerHTML = new DOMParser().parseFromString(frame.srcdoc, "text/html").body.innerHTML;
    fireEvent.load(frame);
    await waitFor(() => expect(frame.contentDocument!.querySelector("img")).toHaveAttribute("src", png));
    expect(frame.contentDocument!.querySelector("img")).toHaveAttribute("alt", "Logo");
    expect(frame.contentDocument!.querySelector("img")).toHaveAttribute("width", "32");
    expect(frame).toHaveAttribute("sandbox", "allow-same-origin allow-scripts");
    expect(frame.srcdoc).toContain('data-theme="light"');
    expect(frame.srcdoc).toContain("script-src 'none'");
    expect(frame.srcdoc).toContain("img-src data:");
    expect(fetch).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Load images" })).toBeNull();
  });

  it.each([
    '<table style="background-color:rgb(255, 238, 204)"><tr><td>Alex</td><td></td></tr></table>',
    '<div style="background-color:rgb(34, 34, 34);color:white"><p>Alex</p></div>',
  ])("uses light rendering while preserving explicitly authored backgrounds: %s", (html) => {
    render(<SignaturePreview html={html} text="Alex" />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    const doc = new DOMParser().parseFromString(frame.srcdoc, "text/html");
    expect(doc.documentElement.getAttribute("data-theme")).toBe("light");
    expect((doc.querySelector("table, .email-root > div") as HTMLElement).style.backgroundColor).toBe(html.includes("table") ? "rgb(255, 238, 204)" : "rgb(34, 34, 34)");
  });

  it("loads hosted previews only after the explicit action and keeps the stored HTML unchanged", async () => {
    const html = '<p>Alex<img src="https://images.example/logo.png" alt="Logo"></p>';
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue(png);
    const { rerender } = render(<SignaturePreview html={html} text="AlexLogo" />);
    const frame = screen.getByTestId("message-body") as HTMLIFrameElement;
    frame.contentDocument!.body.innerHTML = new DOMParser().parseFromString(frame.srcdoc, "text/html").body.innerHTML;
    fireEvent.load(frame);
    expect(frame.contentDocument!.querySelector("img")).not.toHaveAttribute("src");
    expect(fetch).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Load images" }));
    await waitFor(() => expect(frame.contentDocument!.querySelector("img")).toHaveAttribute("src", png));
    expect(fetch).toHaveBeenCalledWith("https://images.example/logo.png");
    rerender(<SignaturePreview html={html} text="AlexLogo" />);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(html).toContain('src="https://images.example/logo.png"');
  });

  it.each([
    '<p>Alex</p><img src="https://tracker.invalid/pixel.png"><img src="file:///private/logo.png"><script>parent.compromised=true</script>',
    '<table><tr><td style="background-image:url(https://tracker.invalid/bg.png)"><img src="data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=" onload="alert(1)"><img src="cid:sender-controlled"></td><td>Alex</td></tr></table>',
  ])("keeps unsafe sources and network capabilities out of a local preview for %s", (html) => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage");
    render(<SignaturePreview html={html} text="Alex" />);
    const doc = new DOMParser().parseFromString((screen.getByTestId("message-body") as HTMLIFrameElement).srcdoc, "text/html");
    expect(doc.body.querySelector("script,[onload],img[src]")).toBeNull();
    expect(doc.body.innerHTML).not.toMatch(/file:|cid:|background-image|svg\+xml/);
    expect(doc.body.textContent).toContain("Alex");
    expect(fetch).not.toHaveBeenCalled();
  });
});
