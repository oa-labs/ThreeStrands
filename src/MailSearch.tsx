import { Bookmark } from "lucide-react";
import { ICON_SIZE } from "./iconSizes";
import { useId, useRef, useState, type ComponentProps } from "react";
import { MESSAGE_FILTER_OPTIONS, type MessageFilterKind } from "./messageFilters";
import { Modal } from "./AppChrome";
import { SearchField } from "./SearchField";
import { saveSearch, type SavedSearch } from "./savedSearches";
import { errorMessage } from "./errors";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

type Props = ComponentProps<typeof SearchField> & {
  accounts: readonly { email: string }[];
  accountId: string | null;
  activeFilters?: ReadonlySet<MessageFilterKind>;
  onManageViews?(): void;
  onCreateSearch?(search: Omit<SavedSearch, "id">): Promise<void>;
};

export function MailSearch({ accounts, accountId, onManageViews, onCreateSearch, activeFilters, ...field }: Props) {
  const [saving, setSaving] = useState<{ query: string; inboxOnly: boolean; accountId: string | null; messageFilters: MessageFilterKind[] } | null>(null);
  return <>
    <SearchField {...field} onSaveSearch={(query) => setSaving({ query, inboxOnly: field.inboxOnly, accountId, messageFilters: [...(activeFilters ?? [])] })} actions={<button type="button" className="btn-icon" aria-label="Saved searches" title="Manage saved searches in Split Inboxes" onClick={onManageViews}><Bookmark size={ICON_SIZE.md} /></button>} />
    {saving ? <SaveSearchDialog accounts={accounts} search={saving} onSave={onCreateSearch} onClose={() => setSaving(null)} onSaved={() => setSaving(null)} /> : null}
  </>;
}

export function SaveSearchDialog({ accounts, search, onClose, onSaved, onSave, initialName = "", editable = false }: {
  accounts: Props["accounts"];
  search: { query: string; inboxOnly: boolean; accountId: string | null; messageFilters: MessageFilterKind[] };
  onClose(): void;
  onSaved(): void;
  onSave?(search: Omit<SavedSearch, "id">): Promise<void> | void;
  initialName?: string;
  editable?: boolean;
}) {
  const [name, setName] = useState(initialName);
  const [query, setQuery] = useState(search.query);
  const [inboxOnly, setInboxOnly] = useState(search.inboxOnly);
  const [filters, setFilters] = useState(search.messageFilters);
  const [busy, setBusy] = useState(false);
  const [scope, setScope] = useState(search.accountId ?? "");
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const descriptionId = useId();
  const validScope = !scope || accounts.some((account) => account.email === scope);
  return <Modal title={editable ? initialName ? "Edit Saved Search" : "New Saved Search" : "Save Search"} initialFocusRef={input} onClose={onClose}>
    <form className="modal-form" onSubmit={(event) => {
      event.preventDefault();
      if (!validScope || !name.trim() || !query.trim() || busy) return;
      try {
        const value = { name: name.trim(), query: query.trim(), accountId: scope || null, inboxOnly, messageFilters: filters };
        if (onSave) {
          setBusy(true);
          Promise.resolve(onSave(value)).then(onSaved).catch((reason) => setError(errorMessage(reason))).finally(() => setBusy(false));
        } else { saveSearch(value); onSaved(); }
      } catch (reason) { setBusy(false); setError(`Search could not be saved: ${errorMessage(reason)}`); }
    }}>
      <label>Name<input {...SEARCH_INPUT_ATTRIBUTES} ref={input} required value={name} onChange={(event) => setName(event.target.value)} /></label>
      <label>Account scope<select value={scope} aria-describedby={descriptionId} onChange={(event) => setScope(event.target.value)}>
        <option value="">All accounts</option>
        {!validScope ? <option value={scope} disabled>{scope} (disconnected)</option> : null}
        {accounts.map((account) => <option key={account.email} value={account.email}>{account.email}</option>)}
      </select></label>
      <p id={descriptionId} className="modal-form-hint">Saved on this computer. All accounts includes accounts you connect later.</p>
      <label>Search<input readOnly={!editable} value={query} onChange={(event) => setQuery(event.target.value)} /></label>
      {editable ? <>
        <label className="modal-form-check"><input type="checkbox" checked={inboxOnly} onChange={(event) => setInboxOnly(event.target.checked)} />Inbox Only</label>
        {MESSAGE_FILTER_OPTIONS.map((option) => <label className="modal-form-check" key={option.kind}><input type="checkbox" checked={filters.includes(option.kind)} onChange={(event) => setFilters((current) => event.target.checked ? [...current, option.kind] : current.filter((kind) => kind !== option.kind))} />{option.label}</label>)}
      </> : null}
      {!editable ? <p>{inboxOnly ? "Inbox Only" : "All mail"}{filters.map((kind) => ` · ${MESSAGE_FILTER_OPTIONS.find((option) => option.kind === kind)?.label}`).join("")}</p> : null}
      {!validScope ? <p role="alert">This account is no longer connected. Choose another account scope.</p> : null}
      {error ? <p role="alert" className="form-error">{error}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={!name.trim() || !query.trim() || !validScope || busy}>{busy ? "Saving…" : "Save Search"}</button>
      </div>
    </form>
  </Modal>;
}
