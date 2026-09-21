import { parseAddress, splitAddressList } from "./emailAddress";

type SnippetVariables = { firstName?: string };

const SNIPPET_TOKENS: Record<string, (vars: SnippetVariables) => string | undefined> = {
  first_name: (vars) => vars.firstName,
};

/** Replaces `{token}` placeholders with resolved values; an unresolved or unknown token is left as-is rather than silently stripped. */
export function renderSnippetBody(body: string, vars: SnippetVariables): string {
  return body.replace(/\{(\w+)\}/g, (match, token: string) => {
    const resolved = SNIPPET_TOKENS[token]?.(vars);
    return resolved ?? match;
  });
}

/** Derives a `{first_name}` value from the first recipient's display name, e.g. "Jane Doe <jane@example.com>" -> "Jane". */
export function firstNameFromRecipient(to: string): string | undefined {
  const first = splitAddressList(to)[0];
  if (!first) return undefined;
  const { name, email } = parseAddress(first);
  if (name === email) return undefined;
  return name.split(" ")[0] || undefined;
}
