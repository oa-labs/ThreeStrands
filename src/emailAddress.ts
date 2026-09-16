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

// Splits an RFC-style address list without treating commas inside quoted
// display names or angle brackets as recipient separators.
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

  return result;
}
