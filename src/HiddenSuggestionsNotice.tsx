/** Validation covers source evidence as well as structured scheduling values. */
export function HiddenSuggestionsNotice({ count }: { count: number }) {
  if (count <= 0) return null;
  return <details className="context-status">
    <summary>{count === 1 ? "1 suggestion couldn’t be verified, so it was hidden." : `${count} suggestions couldn’t be verified, so they were hidden.`}</summary>
    <p>The AI may have returned invalid scheduling information or evidence that could not be matched to the conversation. Open Settings → Diagnostics on this device for recorded rejection reasons. Saved suggestions may predate these diagnostics.</p>
  </details>;
}
