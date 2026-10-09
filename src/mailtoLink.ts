import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Draft } from "./correspondence";
import { logBackgroundFailure } from "./errors";
import { splitAddressList } from "./emailAddress";

/** The draft fields a `mailto:` link may fill in. */
export type MailtoRequest = Pick<Draft, "to" | "cc" | "bcc" | "subject" | "body">;

/** Emitted by the native layer when macOS or the webview hands over a `mailto:` link. */
export const MAIL_LINK_EVENT = "mail-link-received";

const ADDRESS_FIELDS = ["to", "cc", "bcc"] as const;

export function isMailtoLink(href: string): boolean {
  return /^mailto:/i.test(href.trim());
}

// Decodes each run of percent escapes on its own so one malformed escape
// keeps its literal text instead of discarding the whole component. `+` is
// a literal plus in mailto (RFC 6068 §5), never a space.
function decode(value: string): string {
  return value.replace(/(?:%[0-9a-f]{2})+/gi, (run) => {
    try { return decodeURIComponent(run); }
    catch { return run; }
  });
}

function addressList(values: string[]): string {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const value of values) {
    for (const piece of splitAddressList(value.replace(/[\r\n]+/g, " "))) {
      const key = piece.toLocaleLowerCase();
      if (seen.has(key)) continue;
      seen.add(key);
      result.push(piece);
    }
  }
  return result.join(", ");
}

/**
 * Reads a `mailto:` URL (RFC 6068) into draft fields. The link is untrusted:
 * only To, Cc, Bcc, Subject, and Body are honored. Every other header —
 * `attach`, `from`, `in-reply-to`, and so on — is ignored, so a link can
 * never choose local files, the sending identity, or threading. Returns null
 * for anything that is not a mailto link.
 */
export function parseMailto(href: string): MailtoRequest | null {
  const value = href.trim();
  if (!isMailtoLink(value)) return null;
  const rest = value.slice("mailto:".length);
  const queryStart = rest.indexOf("?");
  const addresses: Record<(typeof ADDRESS_FIELDS)[number], string[]> = {
    to: [decode(queryStart < 0 ? rest : rest.slice(0, queryStart))],
    cc: [],
    bcc: [],
  };
  let subject: string | null = null;
  let body: string | null = null;
  if (queryStart >= 0) {
    for (const pair of rest.slice(queryStart + 1).split("&")) {
      if (!pair) continue;
      const separator = pair.indexOf("=");
      const name = decode(separator < 0 ? pair : pair.slice(0, separator)).trim().toLowerCase();
      const fieldValue = separator < 0 ? "" : decode(pair.slice(separator + 1));
      if ((ADDRESS_FIELDS as readonly string[]).includes(name)) addresses[name as keyof typeof addresses].push(fieldValue);
      else if (name === "subject") subject ??= fieldValue;
      else if (name === "body") body ??= fieldValue;
    }
  }
  return {
    to: addressList(addresses.to),
    cc: addressList(addresses.cc),
    bcc: addressList(addresses.bcc),
    subject: (subject ?? "").replace(/[\r\n]+/g, " ").trim(),
    body: (body ?? "").replace(/\r\n?/g, "\n"),
  };
}

type MailtoHandler = (request: MailtoRequest) => void;
let handler: MailtoHandler | null = null;
const waiting: MailtoRequest[] = [];

/**
 * Registers the composer as the destination for mailto links. Links that
 * arrive before a handler is registered wait and are delivered in order.
 */
export function setMailtoHandler(next: MailtoHandler | null): void {
  handler = next;
  if (!next) return;
  for (const request of waiting.splice(0)) next(request);
}

function deliver(href: string): void {
  const request = parseMailto(href);
  if (!request) return;
  if (handler) handler(request);
  else waiting.push(request);
}

/**
 * Opens a link from message content: mailto links start a draft here instead
 * of leaving for the OS mail handler (which may be ThreeStrands itself);
 * everything else opens in the OS browser.
 */
export function openMessageLink(href: string): void {
  if (isMailtoLink(href)) deliver(href);
  else void openUrl(href);
}

/**
 * Drains mailto links the native layer received — from macOS when
 * ThreeStrands is the default mail app, including the one that launched it —
 * now and whenever another arrives. Returns the unsubscribe function.
 */
export function listenForNativeMailLinks(): () => void {
  if (!("__TAURI_INTERNALS__" in window)) return () => {};
  const drain = () => {
    void invoke<string[]>("take_pending_mail_links")
      .then((links) => { for (const link of links) deliver(link); })
      .catch(logBackgroundFailure("receiving mail links"));
  };
  // Drain once the listener is live, so a link queued before then (or
  // between the two) is not left waiting for the next one.
  const listener = listen(MAIL_LINK_EVENT, drain);
  void listener.then(drain);
  return () => { void listener.then((unlisten) => unlisten()); };
}
