import { ChevronDown, ChevronUp, Plus } from "lucide-react";
import { useEffect, useState } from "react";
import { formatLabelName } from "./labels";
import type { Account, Label, SplitInbox, SplitInboxMatchKind } from "./domain";
import { moveItem, useSettingsOperation } from "./settingsOperations";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";

/** Resolves a label id to its display name by scanning every account's label
 * catalog rather than storing the name on the split inbox — ids are stable,
 * but the label could be renamed after the split inbox is created. */
function describeSplitInboxRule(splitInbox: SplitInbox, labelsByAccount: Record<string, Label[]>): string {
  switch (splitInbox.matchKind) {
    case "domain":
      return `Sending domain: ${splitInbox.matchValue}`;
    case "pattern":
      return `Address contains: ${splitInbox.matchValue}`;
    case "label": {
      // Gmail label ids are opaque per account, so only the split's own
      // account's catalog can resolve this id to a real name.
      const label = (labelsByAccount[splitInbox.accountId] ?? []).find(
        (candidate) => candidate.id === splitInbox.matchValue,
      );
      return `Label: ${label ? formatLabelName(label) : splitInbox.matchValue}`;
    }
  }
}

function SplitInboxNameInput({
  splitInbox,
  onCommit,
}: {
  splitInbox: SplitInbox;
  onCommit(name: string): void;
}) {
  const [value, setValue] = useState(splitInbox.name);
  const normalized = value.trim();

  useEffect(() => setValue(splitInbox.name), [splitInbox.name]);

  return (
    <form
      className="account-sender-name"
      onSubmit={(event) => {
        event.preventDefault();
        if (!normalized || normalized === splitInbox.name) return;
        onCommit(normalized);
      }}
    >
      <input
        aria-label={`Name for ${splitInbox.name}`}
        value={value}
        maxLength={200}
        onChange={(event) => setValue(event.target.value)}
      />
      <button className="btn btn-primary" type="submit" disabled={!normalized || normalized === splitInbox.name}>
        Save Name
      </button>
    </form>
  );
}

export function SplitInboxesSettings({
  splitInboxes,
  accounts,
  activeAccountId,
  labelsByAccount,
  onCreate,
  onRename,
  onDelete,
  onReorder,
}: {
  splitInboxes: SplitInbox[];
  accounts: Account[];
  activeAccountId: string | null;
  labelsByAccount: Record<string, Label[]>;
  onCreate(name: string, matchKind: SplitInboxMatchKind, matchValue: string, accountId: string): Promise<void>;
  onRename(id: string, name: string): Promise<void>;
  onDelete(id: string): Promise<void>;
  onReorder(ids: string[]): Promise<void>;
}) {
  const [name, setName] = useState("");
  const [matchKind, setMatchKind] = useState<SplitInboxMatchKind>("domain");
  const [matchValue, setMatchValue] = useState("");
  const [accountId, setAccountId] = useState(() => activeAccountId ?? accounts[0]?.email ?? "");
  const [creating, setCreating] = useState(false);
  const { pending: busyId, error, setError, runFor } = useSettingsOperation();

  // A split inbox belongs to one account, so only that account's labels are
  // valid matches for it.
  const labelOptions = (labelsByAccount[accountId] ?? [])
    .filter((label) => label.kind === "user")
    .sort((a, b) => formatLabelName(a).localeCompare(formatLabelName(b), undefined, { sensitivity: "base" }));


  const move = (index: number, direction: -1 | 1) => {
    const next = moveItem(splitInboxes, index, direction);
    if (next) runFor(splitInboxes[index]!.id, () => onReorder(next.map((splitInbox) => splitInbox.id)));
  };

  return (
    <section className="settings-section accounts-manager" aria-label="Split Inboxes">
      <div className="accounts-manager-header">
        <div>
          <h3>Split Inboxes</h3>
          <p>
            Show a filtered slice of your inbox as its own view in the sidebar —
            for example, every message from one client's sending domain.
          </p>
        </div>
      </div>
      <form
        className="split-inbox-add"
        onSubmit={(event) => {
          event.preventDefault();
          const normalizedName = name.trim();
          const normalizedValue = matchValue.trim();
          if (!normalizedName || !normalizedValue || !accountId || creating) return;
          setCreating(true);
          setError(null);
          void onCreate(normalizedName, matchKind, normalizedValue, accountId)
            .then(() => {
              setName("");
              setMatchValue("");
            })
            .catch((reason: unknown) => setError(errorMessage(reason)))
            .finally(() => setCreating(false));
        }}
      >
        <input
          value={name}
          placeholder="Name (e.g. Acme Corp)"
          aria-label="Split Inbox Name"
          onChange={(event) => setName(event.target.value)}
        />
        <select
          value={accountId}
          aria-label="Account"
          onChange={(event) => {
            setAccountId(event.target.value);
            setMatchValue("");
          }}
        >
          {accounts.map((account) => (
            <option key={account.email} value={account.email}>{account.email}</option>
          ))}
        </select>
        <select
          value={matchKind}
          aria-label="Match By"
          onChange={(event) => {
            setMatchKind(event.target.value as SplitInboxMatchKind);
            setMatchValue("");
          }}
        >
          <option value="domain">Sending domain</option>
          <option value="label">Label</option>
          <option value="pattern">Address contains</option>
        </select>
        {matchKind === "label" ? (
          <select value={matchValue} aria-label="Label" onChange={(event) => setMatchValue(event.target.value)}>
            <option value="" disabled>Choose a label</option>
            {labelOptions.map((label) => (
              <option key={label.id} value={label.id}>{formatLabelName(label)}</option>
            ))}
          </select>
        ) : (
          <input
            value={matchValue}
            placeholder={matchKind === "domain" ? "acme.com" : "boss@"}
            aria-label={matchKind === "domain" ? "Sending Domain" : "Address Pattern"}
            onChange={(event) => setMatchValue(event.target.value)}
          />
        )}
        <button type="submit" className="btn btn-primary" disabled={creating || !name.trim() || !matchValue.trim() || !accountId}>
          <Plus size={ICON_SIZE.md} />
          {creating ? "Adding…" : "Add Split Inbox"}
        </button>
      </form>
      {splitInboxes.length === 0 ? (
        <div className="accounts-empty">
          <strong>No split inboxes yet</strong>
          <p>Add one above to see a filtered slice of your inbox in the sidebar.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {splitInboxes.map((splitInbox, index) => (
            <li className="account-card" key={splitInbox.id}>
              <div className="account-card-row">
                <div className="account-card-identity">
                  <SplitInboxNameInput
                    splitInbox={splitInbox}
                    onCommit={(nextName) => runFor(splitInbox.id, () => onRename(splitInbox.id, nextName))}
                  />
                  <span className="account-card-email">
                    {describeSplitInboxRule(splitInbox, labelsByAccount)} — {splitInbox.accountId}
                  </span>
                </div>
              </div>
              <div className="account-card-controls">
                <span className="accounts-list-actions">
                  <span className="account-reorder">
                    <button className="btn-icon btn-icon-sm"
                      type="button"
                      aria-label={`Move ${splitInbox.name} up`}
                      disabled={index === 0 || busyId !== null}
                      onClick={() => move(index, -1)}
                    >
                      <ChevronUp size={ICON_SIZE.sm} />
                    </button>
                    <button className="btn-icon btn-icon-sm"
                      type="button"
                      aria-label={`Move ${splitInbox.name} down`}
                      disabled={index === splitInboxes.length - 1 || busyId !== null}
                      onClick={() => move(index, 1)}
                    >
                      <ChevronDown size={ICON_SIZE.sm} />
                    </button>
                  </span>
                  <button
                    type="button"
                    className="btn btn-sm btn-danger"
                    disabled={busyId !== null}
                    onClick={() => runFor(splitInbox.id, () => onDelete(splitInbox.id))}
                  >
                    Delete
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}
