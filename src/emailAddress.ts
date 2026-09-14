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
