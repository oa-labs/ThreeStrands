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
  it("uses the sandboxed preview, omits remote images by default and sanitizes imported content", async () => {
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
    expect(onImport.mock.calls[0][0]).not.toMatch(/script|img|https/);
  });

  it("downloads opted-in images through the native proxy and keeps HTML when a source is blocked", async () => {
    const fetch = vi.spyOn(mailClient, "fetchRemoteImage").mockRejectedValue(new Error("Image source is blocked"));
    const onImport = vi.fn();
    render(<SignatureImportDialog onClose={vi.fn()} onImport={onImport} />);
    const source = '<p>Alex</p><img src="https://images.example/logo.png" alt="Logo">';
    fireEvent.change(screen.getByLabelText("HTML"), { target: { value: source } });
    fireEvent.click(screen.getByLabelText("Embed Remote Images"));
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Image source is blocked");
    expect(screen.getByLabelText("HTML")).toHaveValue(source); expect(onImport).not.toHaveBeenCalled();
    fetch.mockResolvedValue("data:image/png;base64,aA==");
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() => expect(onImport).toHaveBeenCalledWith('<p>Alex</p><img src="data:image/png;base64,aA==" alt="Logo">'));
    expect(fetch).toHaveBeenCalledWith("https://images.example/logo.png");
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
