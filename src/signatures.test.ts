import { describe, expect, it } from "vitest";
import { defaultSignature, readSignatureEditor, readSignatureHtml, signatureEditorHtml, sanitizeSignatureHtml, signatureBody, signatureDefaultsKey, validateSignature } from "./signatures";
import { EMAIL_CSS_LIMITS, SIGNATURE_LIMITS } from "./emailRenderingPolicy";
import type { Draft } from "./correspondence";

function node(html: string) { const root = document.createElement("div"); root.innerHTML = html; return root; }

describe("signature content and portability", () => {
  it.each([
    '<table cellpadding="0" cellspacing="8" style="border-collapse:separate"><tr><td width="32"><img src="data:image/png;base64,aA==" width="24" alt="Logo"></td><td style="padding-left:12px;color:#245789;font-size:14px">Alex<br><a href="mailto:alex@example.com">Email</a></td><td> </td></tr></table>',
    '<div style="display:flex;gap:12px;align-items:center"><div style="border-right:1px solid #ccc;padding-right:8px"><b>Alex</b><p></p></div><div><span style="font-family:serif;font-style:italic">Designer</span><br><a href="https://example.com">Website</a></div></div>',
  ])("preserves safe authored structure and formatting across save and reopen: %s", (fixture) => {
    const html = sanitizeSignatureHtml(fixture);
    expect(sanitizeSignatureHtml(html)).toBe(html);
    const root = node(html);
    if (root.querySelector("table")) {
      expect(root.querySelectorAll("td")).toHaveLength(3);
      expect(root.querySelector("table")).toHaveAttribute("cellspacing", "8");
      expect(root.querySelector("img")).toHaveAttribute("width", "24");
      expect(root.querySelectorAll("td")[1].style.paddingLeft).toBe("12px");
      expect(readSignatureEditor(root).text).toContain("Logo");
    } else {
      expect(root.firstElementChild).toHaveStyle({ display: "flex", gap: "12px" });
      expect(root.querySelector("p")).not.toBeNull();
    }
    expect(readSignatureEditor(root).text).toContain("Alex");
  });

  it("keeps hosted sources inert and removes active markup and app-escaping styles before insertion", () => {
    const root = node(signatureEditorHtml('<script>alert(1)</script><style>@import "https://evil.invalid";</style><iframe src="https://evil.invalid"></iframe><form action="https://evil.invalid"><input></form><svg onload="alert(1)"></svg><span id="app" data-secret="x" onclick="alert(1)" style="position:fixed;z-index:999999;top:0;background:url(https://evil.invalid/x);color:red">Name</span><a href="javascript:alert(1)" target="_top">Bad</a><img src="https://evil.invalid/x" onerror="alert(1)"><img src="file:///secret"><img src="data:image/svg+xml;base64,PHN2Zz4=">'));
    expect(root.querySelector("script,style,iframe,form,input,svg,img[src]")).toBeNull();
    expect(root.innerHTML).not.toMatch(/onclick|onload|onerror|data-secret|id=|url\(|position|z-index|target|javascript/);
    expect((root.querySelector("span") as HTMLElement).style.color).toBe("red");
    expect(root.textContent).toContain("Name");
  });

  it.each([
    '<table><tr><td><img src="https://cdn.example/logo.png?version=2&amp;size=large" alt="Logo" width="120"></td><td>Alex</td><td></td></tr></table>',
    '<div style="display:flex"><img src="https://images.example/large.gif" alt="Team" height="80"><p>Alex</p></div>',
  ])("preserves hosted image URLs through editing and plaintext conversion: %s", (html) => {
    const canonical = sanitizeSignatureHtml(html);
    const editor = node(signatureEditorHtml(html));
    expect(editor.querySelector("img")).not.toHaveAttribute("src");
    expect(editor.querySelector("img")).toHaveAttribute("data-compose-source");
    const saved = readSignatureEditor(editor);
    expect(saved.html).toBe(canonical);
    expect(saved.html).not.toContain("data-compose");
    expect(saved.text).toContain("Alex");
    expect(readSignatureHtml(saved.html)).toEqual(saved);
  });
  it.each(["http://images.example/logo.png", "//images.example/logo.png", "https://user:secret@images.example/logo.png", "https://", "https://images.example/\nlogo.png", "javascript:alert(1)", "file:///logo.png"]) ("rejects nonportable or credential-bearing hosted sources: %s", (source) => {
    expect(node(signatureEditorHtml(`<img src="${source}">`)).querySelector("img")).toBeNull();
  });

  it("rejects CID and blob references in portable templates but retains native draft CIDs", () => {
    const html = '<img src="cid:logo@local" alt="Logo"><img src="blob:unportable" alt="Other">';
    expect(node(sanitizeSignatureHtml(html)).querySelector("img")).toBeNull();
    expect(node(sanitizeSignatureHtml(html, true)).querySelectorAll("img")).toHaveLength(1);
  });

  it.each([EMAIL_CSS_LIMITS.maxAbsolutePx - 1, EMAIL_CSS_LIMITS.maxAbsolutePx, EMAIL_CSS_LIMITS.maxAbsolutePx + 1])("uses the shared dimension boundary at %s", (width) => {
    const root = node(sanitizeSignatureHtml(`<table width="${width}" style="width:${width}px"><tr><td></td></tr></table>`));
    const table = root.querySelector("table")!;
    expect(table.hasAttribute("width")).toBe(width <= EMAIL_CSS_LIMITS.maxAbsolutePx);
    expect(Boolean(table.style.width)).toBe(width <= EMAIL_CSS_LIMITS.maxAbsolutePx);
  });

  it("retains sizes emitted by the editor's legacy font command", () => {
    const root = node(sanitizeSignatureHtml('<font face="serif" color="#123456" size="4">Text</font>'));
    expect(root.firstElementChild).toHaveStyle({ fontFamily: "serif", fontSize: "18px" });
    expect(root.firstElementChild).not.toHaveAttribute("size");
  });
  it("keeps labeled contact links useful in the plain-text alternative without changing HTML", () => {
    const html = '<p><a href="https://example.com">Website</a><br><a href="mailto:alex@example.com">alex@example.com</a><br><a href="tel:+15551234567">Call</a></p>';
    const content = readSignatureEditor(node(html));
    expect(content.html).toBe(html);
    expect(content.text).toBe("Website (https://example.com)\nalex@example.com\nCall (+15551234567)");
  });

  it.each([SIGNATURE_LIMITS.maxHtmlBytes - 1, SIGNATURE_LIMITS.maxHtmlBytes, SIGNATURE_LIMITS.maxHtmlBytes + 1])("enforces portable UTF-8 bytes at %s", (size) => {
    const signature = { id: "s", name: "Work", html: "a".repeat(size), text: "Work" };
    if (size <= SIGNATURE_LIMITS.maxHtmlBytes) expect(() => validateSignature(signature)).not.toThrow();
    else expect(() => validateSignature(signature)).toThrow(/too large/);
    expect(() => validateSignature({ ...signature, html: "é".repeat(size) })).toThrow(/too large/);
  });
  it.each([SIGNATURE_LIMITS.maxTextBytes - 1, SIGNATURE_LIMITS.maxTextBytes, SIGNATURE_LIMITS.maxTextBytes + 1])("bounds the plain-text alternative at %s bytes", (size) => {
    const signature = { id: "s", name: "Work", html: "<p>Work</p>", text: "a".repeat(size) };
    if (size <= SIGNATURE_LIMITS.maxTextBytes) expect(() => validateSignature(signature)).not.toThrow();
    else expect(() => validateSignature(signature)).toThrow(/too large/);
  });
  it.each([SIGNATURE_LIMITS.maxNameLength - 1, SIGNATURE_LIMITS.maxNameLength, SIGNATURE_LIMITS.maxNameLength + 1])("counts Unicode characters in names at %s", (length) => {
    const signature = { id: "s", name: "😀".repeat(length), html: "<p>Work</p>", text: "Work" };
    if (length <= SIGNATURE_LIMITS.maxNameLength) expect(() => validateSignature(signature)).not.toThrow();
    else expect(() => validateSignature(signature)).toThrow(/signature name/);
  });
});

describe("signature selection and message assembly", () => {
  const full = { id: "full", name: "Full", html: "<p>Alex Example</p>", text: "Alex Example" };
  const short = { id: "short", name: "Short", html: "<p>Alex</p>", text: "Alex" };
  const base = { id: signatureDefaultsKey("me@example.com"), account: "me@example.com", address: "", newSignatureId: "full", replySignatureId: "short" };
  const library = { signatures: [full, short], defaults: [base] };
  it("resolves each compose mode and inherits aliases without confusing None with inheritance", () => {
    expect(defaultSignature(library, "ME@example.com", "alias@example.com", "new")?.id).toBe("full");
    for (const mode of ["reply", "replyAll", "forward"] as const) expect(defaultSignature(library, "me@example.com", "alias@example.com", mode)?.id).toBe("short");
    const override = { ...base, id: signatureDefaultsKey(base.account, "alias@example.com"), address: "alias@example.com", newSignatureId: null };
    expect(defaultSignature({ ...library, defaults: [base, override] }, base.account, override.address, "new")).toBeNull();
    expect(defaultSignature({ ...library, signatures: [short] }, base.account, base.account, "new")).toBeNull();
  });
  it("assembles a saved snapshot above history and ignores later template edits", () => {
    const draft = { body: "Hi 😀\n\nQuote", bodyHtml: "Hi 😀<br><br>Quote", signature: { ...full, automatic: true, textOffset: 5, htmlOffset: 5 } } as Draft;
    expect(signatureBody(draft)).toEqual({ text: "Hi 😀\n\nAlex Example\n\nQuote", html: "Hi 😀<br><br><p>Alex Example</p><br><br>Quote" });
    const changed = { ...library, signatures: [] };
    expect(defaultSignature(changed, base.account, base.account, "new")).toBeNull();
    expect(signatureBody(draft).text).toContain("Alex Example");
  });
});
