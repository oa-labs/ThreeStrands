export interface ParsedAddress {
  name: string;
  email: string;
}

// RFC 5322 allows a display name to be a quoted-string ("Doe, Jane") with
// backslash-escaped '"' and '\' inside it. Strip that quoting so the UI shows
// the plain name rather than the wire encoding.
function normalizeDisplayName(value: string): string {
  const trimmed = value.trim();
  const unquoted =
    trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"')
      ? trimmed.slice(1, -1).replace(/\\(["\\])/g, "$1")
      : trimmed;
  return unquoted.replace(/\s+/g, " ").trim();
}

// Parses a "From"/"To"-style header value ("Display Name <email@example.com>"
// or a bare address) into a display name and email. Falls back to the email
// (or the raw value) when no display name is present.
export function parseAddress(value: string): ParsedAddress {
  const trimmed = value.trim();
  const openBracket = trimmed.lastIndexOf("<");
  const closeBracket = trimmed.lastIndexOf(">");

  if (openBracket >= 0 && closeBracket > openBracket) {
    const email = trimmed.slice(openBracket + 1, closeBracket).trim();
    const name = normalizeDisplayName(trimmed.slice(0, openBracket));
    if (email) return { name: name || email, email };
  }

  return { name: normalizeDisplayName(trimmed), email: trimmed };
}

// Preserve the complete sender-authored display name. Delegated senders can
// arrive as a single-quoted name followed by "via ..."; that suffix describes
// the delivery path rather than the sender identity, so omit it from the UI.
export function formatDisplayName(value: string): string {
  const name = value.trim();
  const delegatedName = name.match(/^'(.+)'\s+via\s+.+$/i)?.[1];

  return delegatedName?.trim() || name;
}

// A loose "is this a full address yet" check, not RFC 5322 validation — just
// enough to decide whether a segment is a committed recipient or still being
// typed. The server validates for real at send time.
export function looksLikeCompleteAddress(email: string): boolean {
  const at = email.indexOf("@");
  if (at <= 0) return false;
  const domain = email.slice(at + 1);
  return domain.includes(".") && !domain.startsWith(".") && !domain.endsWith(".");
}

// Splits an RFC-style address list without treating commas inside quoted
// display names or angle brackets as recipient separators. Stored mail can
// also carry an unquoted comma in a display name ("Daniel O'Connor, CFA®
// <dan@example.com>"); a piece holding no address joins the "Name <address>"
// piece after it rather than becoming a recipient of its own.
export function splitAddressList(value: string): string[] {
  const result: string[] = [];
  let start = 0;
  let quoted = false;
  let angleDepth = 0;

  for (let index = 0; index <= value.length; index++) {
    const character = value[index];
    if (character === '"' && value[index - 1] !== "\\") quoted = !quoted;
    else if (!quoted && character === "<") angleDepth++;
    else if (!quoted && character === ">") angleDepth = Math.max(0, angleDepth - 1);

    if (index === value.length || (character === "," && !quoted && angleDepth === 0)) {
      const address = value.slice(start, index).trim();
      if (address) result.push(address);
      start = index + 1;
    }
  }

  const merged: string[] = [];
  let pendingName = "";
  for (const piece of result) {
    if (!piece.includes("@")) {
      pendingName = pendingName ? `${pendingName}, ${piece}` : piece;
    } else if (pendingName && piece.includes("<")) {
      merged.push(`${pendingName}, ${piece}`);
      pendingName = "";
    } else {
      if (pendingName) merged.push(pendingName);
      merged.push(piece);
      pendingName = "";
    }
  }
  if (pendingName) merged.push(pendingName);
  return merged;
}

// RFC 5322 specials. A display name holding any of them must be a quoted
// string, or a comma in "Doe, Jane" splits one recipient into two.
const DISPLAY_NAME_SPECIALS = /[()<>[\]:;@\\,."]/;

/** One recipient as header text: `Name <email>`, quoting the name when it needs it, or the bare address. */
export function formatAddress(name: string | null, email: string): string {
  const display = name?.replace(/\s+/g, " ").trim();
  if (!display || display.toLocaleLowerCase() === email.toLocaleLowerCase()) return email;
  return `${DISPLAY_NAME_SPECIALS.test(display) ? `"${display.replace(/(["\\])/g, "\\$1")}"` : display} <${email}>`;
}

/**
 * Re-encodes every complete recipient in a list with `formatAddress`, so an
 * unquoted comma in a name (from older drafts or pasted text) no longer reads
 * as a separator. Pieces without an address are kept as written, and so is a
 * trailing separator.
 */
export function normalizeAddressList(value: string): string {
  const pieces = splitAddressList(value).map((segment) => {
    const parsed = parseAddress(segment);
    const email = parsed.email.trim();
    return email.includes("@") ? formatAddress(parsed.name === parsed.email ? null : parsed.name, email) : segment.trim();
  });
  if (!pieces.length) return value;
  return /,\s*$/.test(value) ? `${pieces.join(", ")}, ` : pieces.join(", ");
}
