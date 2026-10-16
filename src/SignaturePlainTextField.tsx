/** Plain text is an independent email alternative once the user edits it. */
export function SignaturePlainTextField({ text, busy, onChange, onRegenerate }: {
  text: string; busy: boolean; onChange(text: string): void; onRegenerate(): void;
}) {
  return <section aria-label="Plain Text Preview" className="signature-plain-field">
    <label className="snippet-field"><span>Plain Text</span><textarea rows={6} value={text} disabled={busy} onChange={(event) => onChange(event.target.value)} /></label>
    <p className="settings-hint">Used by email clients that display plain text. Editing it leaves the visual signature unchanged.</p>
    <div className="signature-actions"><button type="button" className="btn btn-sm" disabled={busy} onClick={onRegenerate}>Regenerate from Signature</button></div>
  </section>;
}
