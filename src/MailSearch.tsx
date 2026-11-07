import { Bookmark } from "lucide-react";
import { ICON_SIZE } from "./iconSizes";
import { useId, useRef, useState, type ComponentProps } from "react";
import { MESSAGE_FILTER_OPTIONS, type MessageFilterKind } from "./messageFilters";
import { Modal } from "./AppChrome";
import { SearchField } from "./SearchField";
import { deleteSavedSearch, readSavedSearches, saveSearch, type SavedSearch } from "./savedSearches";
import { errorMessage } from "./errors";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

type Props = ComponentProps<typeof SearchField> & {
  accounts: readonly { email: string }[];
  accountId: string | null;
  activeFilters?: ReadonlySet<MessageFilterKind>;
  onOpenSavedSearch(search: SavedSearch): void;
};

export function MailSearch({ accounts, accountId, onOpenSavedSearch, activeFilters, ...field }: Props) {
  const [catalogOpen, setCatalogOpen] = useState(false);
  const [saving, setSaving] = useState<{ query: string; inboxOnly: boolean; accountId: string | null; messageFilters: MessageFilterKind[] } | null>(null);
  const [searches, setSearches] = useState<SavedSearch[]>([]);
  const [error, setError] = useState<string | null>(null);
  const scopeLabel = (scope: string | null) => scope ?? "All accounts";
  return <>
    <SearchField {...field} onSaveSearch={(query) => setSaving({ query, inboxOnly: field.inboxOnly, accountId, messageFilters: [...(activeFilters ?? [])] })} />
    <button type="button" className="btn-icon" aria-label="Saved searches" title="Saved searches" onClick={() => {
      setError(null);
      try { setSearches(readSavedSearches()); } catch (reason) { setError(errorMessage(reason)); }
      setCatalogOpen(true);
    }}><Bookmark size={ICON_SIZE.md} /></button>
    {saving ? <SaveSearchDialog accounts={accounts} search={saving} onClose={() => setSaving(null)} onSaved={() => setSaving(null)} /> : null}
    {catalogOpen ? <Modal title="Saved searches" onClose={() => setCatalogOpen(false)}>
      <div className="modal-form">
        <p className="modal-form-hint">Saved on this computer. All accounts includes accounts you connect later.</p>
        {error ? <p role="alert" className="form-error">{error}</p> : null}
        {!error && searches.length === 0 ? <p>No saved searches yet. Search your mail, then choose Save Search.</p> : null}
        <ul className="saved-search-list">
          {searches.map((search) => {
            const available = search.accountId === null || accounts.some((account) => account.email === search.accountId);
            return <li key={search.id}>
              <button type="button" className="btn" disabled={!available} onClick={() => {
                onOpenSavedSearch(search);
                setCatalogOpen(false);
              }}>{search.name} · {scopeLabel(search.accountId)}{search.inboxOnly ? " · Inbox Only" : ""}</button>
              <small>{search.query}{search.messageFilters.map((kind) => ` · ${MESSAGE_FILTER_OPTIONS.find((option) => option.kind === kind)?.label}`).join("")}{!available ? " · Account disconnected" : ""}</small>
              <button type="button" className="btn" aria-label={`Delete saved search ${search.name} for ${scopeLabel(search.accountId)}`} onClick={() => {
                try { setSearches(deleteSavedSearch(search.id)); setError(null); } catch (reason) { setError(errorMessage(reason)); }
              }}>Delete</button>
            </li>;
          })}
        </ul>
        <div className="modal-form-actions"><button type="button" className="btn" onClick={() => setCatalogOpen(false)}>Close</button></div>
      </div>
    </Modal> : null}
  </>;
}

function SaveSearchDialog({ accounts, search, onClose, onSaved }: {
  accounts: Props["accounts"];
  search: { query: string; inboxOnly: boolean; accountId: string | null; messageFilters: MessageFilterKind[] };
  onClose(): void;
  onSaved(): void;
}) {
  const [name, setName] = useState("");
  const [scope, setScope] = useState(search.accountId ?? "");
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const descriptionId = useId();
  const validScope = !scope || accounts.some((account) => account.email === scope);
  return <Modal title="Save Search" initialFocusRef={input} onClose={onClose}>
    <form className="modal-form" onSubmit={(event) => {
      event.preventDefault();
      if (!validScope) return;
      try {
        saveSearch({ name, query: search.query, accountId: scope || null, inboxOnly: search.inboxOnly, messageFilters: search.messageFilters });
        onSaved();
      } catch (reason) { setError(`Search could not be saved: ${errorMessage(reason)}`); }
    }}>
      <label>Name<input {...SEARCH_INPUT_ATTRIBUTES} ref={input} required value={name} onChange={(event) => setName(event.target.value)} /></label>
      <label>Account scope<select value={scope} aria-describedby={descriptionId} onChange={(event) => setScope(event.target.value)}>
        <option value="">All accounts</option>
        {accounts.map((account) => <option key={account.email} value={account.email}>{account.email}</option>)}
      </select></label>
      <p id={descriptionId} className="modal-form-hint">Saved on this computer. All accounts includes accounts you connect later.</p>
      <label>Search<input readOnly value={search.query} /></label>
      <p>{search.inboxOnly ? "Inbox Only" : "All mail"}{search.messageFilters.map((kind) => ` · ${MESSAGE_FILTER_OPTIONS.find((option) => option.kind === kind)?.label}`).join("")}</p>
      {!validScope ? <p role="alert">This account is no longer connected. Choose another account scope.</p> : null}
      {error ? <p role="alert" className="form-error">{error}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={!name.trim() || !validScope}>Save Search</button>
      </div>
    </form>
  </Modal>;
}
