import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { SignaturesSettings } from "./SignaturesSettings";
import { SignatureEditor, SignatureImportDialog } from "./SignatureEditor";
import { mailClient } from "./data/client";
import { signatureDefaultsKey, type Signature, type SignatureState } from "./signatures";
import type { Account } from "./domain";
import { SIGNATURE_LIMITS } from "./emailRenderingPolicy";

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
  it("uses the sandboxed preview, omits remote images when embedding is turned off and sanitizes imported content", async () => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage"); const onImport = vi.fn();
    render(<SignatureImportDialog onClose={vi.fn()} onImport={onImport} />);
    const html = screen.getByLabelText("HTML"); expect(html).toHaveFocus();
    fireEvent.change(html, { target: { value: '<table><tr><td>Alex</td><td></td></tr></table><img src="https://images.example/logo.png"><script>alert(1)</script>' } });
    const preview = screen.getByRole("region", { name: "Signature Preview" });
    expect(preview.querySelector("iframe")).toHaveAttribute("sandbox", "allow-same-origin allow-scripts");
    expect(preview.querySelector("iframe")?.srcdoc).toContain("script-src 'none'");
    expect(screen.getByLabelText("Embed Remote Images")).toBeChecked();
    fireEvent.click(screen.getByLabelText("Embed Remote Images"));
    expect(fetch).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(onImport).toHaveBeenCalled());
    expect(onImport.mock.calls[0][0]).toContain("<td></td>");
    expect(onImport.mock.calls[0][0]).not.toMatch(/script|img|https/);
  });

  it("downloads opted-in images through the native proxy and keeps HTML when a source is blocked", async () => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockRejectedValue(new Error("Image source is blocked"));
    const onImport = vi.fn();
    render(<SignatureImportDialog onClose={vi.fn()} onImport={onImport} />);
    const source = '<p>Alex</p><img src="https://images.example/logo.png" alt="Logo">';
    fireEvent.change(screen.getByLabelText("HTML"), { target: { value: source } });
    expect(screen.getByLabelText("Embed Remote Images")).toBeChecked();
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

  it("downloads pasted remote images through the native proxy and embeds them in the signature", async () => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue("data:image/png;base64,aA==");
    // Two structurally different webmail signatures: a table layout and a flow layout repeating one logo.
    const editor = pasteSignature('<table><tr><td><img src="https://images.example/logo.png" alt="Logo" width="80"></td><td>Alex</td></tr></table><div style="background-image:url(https://images.example/bg.png)">x</div>');
    expect(screen.getByRole("status")).toHaveTextContent("Downloading images…");
    await waitFor(() => expect(editor.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,aA=="));
    expect(editor.querySelector("img")).toHaveAttribute("alt", "Logo");
    expect(editor.querySelector("img")).toHaveAttribute("width", "80");
    expect(editor.innerHTML).not.toContain("https://");
    expect(fetch.mock.calls).toEqual([["https://images.example/logo.png"]]);
    expect(screen.queryByRole("status")).toBeNull(); expect(screen.queryByRole("alert")).toBeNull();
    cleanup(); fetch.mockClear();
    const flow = pasteSignature('<p><img src="https://cdn.example/a.gif"> Alex Doe</p><p><a href="https://example.com"><img src="https://cdn.example/a.gif"></a> <script>alert(1)</script></p>');
    await waitFor(() => expect(flow.querySelectorAll("img")).toHaveLength(2));
    expect(fetch.mock.calls).toEqual([["https://cdn.example/a.gif"]]);
    expect(flow.innerHTML).not.toMatch(/script|cdn\.example/);
  });

  it("keeps the rest of a pasted signature and explains when an image can't be downloaded", async () => {
    vi.spyOn(mailClient, "fetchRemoteImage").mockImplementation(async (url) => {
      if (url.includes("blocked")) throw new Error("Image source is blocked");
      return "data:image/png;base64,aA==";
    });
    const editor = pasteSignature('<p>Alex</p><img src="https://images.example/blocked.png"><img src="https://images.example/ok.png">');
    expect(await screen.findByRole("alert")).toHaveTextContent("An image couldn't be added. Image source is blocked. Use Add Image… to add it from your computer.");
    expect(editor).toHaveTextContent("Alex");
    expect(Array.from(editor.querySelectorAll("img"), (image) => image.getAttribute("src"))).toEqual(["data:image/png;base64,aA=="]);
  });

  it.each([SIGNATURE_LIMITS.maxImageBytes - 1, SIGNATURE_LIMITS.maxImageBytes, SIGNATURE_LIMITS.maxImageBytes + 1])("bounds pasted remote images at %s bytes like uploads", async (size) => {
    vi.spyOn(mailClient, "fetchRemoteImage").mockResolvedValue(pngOfBytes(size));
    const editor = pasteSignature('<p>Alex</p><img src="https://images.example/logo.png">');
    await waitFor(() => expect(editor).toHaveTextContent("Alex"));
    if (size <= SIGNATURE_LIMITS.maxImageBytes) {
      expect(editor.querySelector("img")?.getAttribute("src")).toBe(pngOfBytes(size));
      expect(screen.queryByRole("alert")).toBeNull();
    } else {
      expect(editor.querySelector("img")).toBeNull();
      expect(screen.getByRole("alert")).toHaveTextContent("larger than 128 KiB");
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
    expect(signature.html).toContain("<table>"); expect(signature.html).not.toMatch(/script|onclick|background-image|<img/i);
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
    expect(screen.getByRole("region", { name: "Plain Text Preview" })).toHaveTextContent("Logo");
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0][1].html).toContain('src="data:image/png;base64,aA=="');
    expect(fetch.mock.calls).toEqual([["https://images.example/logo.png"], ["https://images.example/logo.png"]]);
    expect(save.mock.calls[0][2]).toBe(false);
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
