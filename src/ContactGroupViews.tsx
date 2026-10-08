import { useEffect, useId, useState } from "react";
import { Check, Plus, Trash2, UserPlus, Users, X } from "lucide-react";
import { Modal } from "./AppChrome";
import type { ContactGroup, ContactProfile } from "./domain";
import { errorMessage } from "./errors";
import { FindOrCreatePicker } from "./FindOrCreatePicker";
import { InlineConfirm } from "./InlineConfirm";
import { ICON_SIZE } from "./iconSizes";
import { MAX_CONTACT_GROUP_NAME, groupCandidates, groupMembers, memberCountLabel, typedAddress } from "./contactGroups";

const contactName = (profile: ContactProfile) => profile.displayName || profile.addresses[0] || "Unnamed contact";

function ContactAvatar({ profile }: { profile: ContactProfile }) {
  return <span className="contact-avatar small">
    {profile.photoData ? <img src={`data:image/jpeg;base64,${profile.photoData}`} alt="" /> : <span>{contactName(profile).slice(0, 1).toLocaleUpperCase()}</span>}
  </span>;
}

/** The Groups view's list pane: every group by name, and an inline form to create one. */
export function ContactGroupList({
  groups,
  selectedId,
  busy,
  searching,
  onSelect,
  onCreate,
}: {
  groups: readonly ContactGroup[];
  selectedId: string | null;
  busy: boolean;
  searching: boolean;
  onSelect(id: string): void;
  onCreate(name: string): Promise<boolean>;
}) {
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const submit = async () => {
    if (!name.trim()) return;
    if (await onCreate(name)) {
      setName("");
      setCreating(false);
    }
  };
  return <>
    <div className="contacts-list-toolbar">
      <p className="contacts-sort-hint">By name · use a group to email everyone in it</p>
      <button type="button" className="btn-link contacts-select-toggle" aria-expanded={creating} onClick={() => setCreating((current) => !current)}>New Group</button>
    </div>
    {creating ? <form className="contact-group-create" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <input
        autoFocus
        aria-label="New group name"
        placeholder="Group name"
        maxLength={MAX_CONTACT_GROUP_NAME}
        value={name}
        disabled={busy}
        onChange={(event) => setName(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          event.preventDefault();
          event.stopPropagation();
          setCreating(false);
          setName("");
        }}
      />
      <button type="submit" className="btn btn-sm btn-primary" disabled={busy || !name.trim()}>Create</button>
    </form> : null}
    <div className="contacts-list">
      {groups.length ? <section className="contact-list-group" aria-label="Groups"><h2>Groups</h2>
        {groups.map((group) => <button
          key={group.id}
          type="button"
          data-mailbox-tab-shortcut
          className={`contact-list-item${selectedId === group.id ? " selected" : ""}`}
          aria-pressed={selectedId === group.id}
          onClick={() => onSelect(group.id)}
        >
          <span className="contact-avatar small contact-group-avatar"><Users size={ICON_SIZE.sm} /></span>
          <span className="contact-list-copy"><strong>{group.name}</strong><small>{memberCountLabel(group.memberIds.length)}</small></span>
        </button>)}
      </section> : null}
    </div>
    {!groups.length ? <p className="contacts-empty">{searching ? "No groups match this search." : "No groups yet. Create one, or use Select in All Contacts to add several people to a group at once."}</p> : null}
  </>;
}

/** One group: rename it, see and change its members, or delete it. */
export function ContactGroupDetail({
  group,
  profiles,
  busy,
  onOpenContact,
  onRename,
  onDelete,
  onAddMembers,
  onRemoveMember,
}: {
  group: ContactGroup;
  /** Every contact the address book knows, saved and mail-derived. */
  profiles: readonly ContactProfile[];
  busy: boolean;
  onOpenContact(id: string): void;
  onRename(name: string): Promise<boolean>;
  onDelete(): Promise<void>;
  onAddMembers(contactIds: string[], emails: string[]): Promise<void>;
  onRemoveMember(contactId: string): Promise<void>;
}) {
  const [name, setName] = useState(group.name);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [adding, setAdding] = useState(false);
  // Keyed by group id in the workspace, so only a rename needs syncing here.
  useEffect(() => setName(group.name), [group.name]);
  const members = groupMembers(group, profiles);
  const rename = async () => {
    if (name.trim() === group.name) {
      setName(group.name);
      return;
    }
    if (!(await onRename(name))) setName(group.name);
  };
  return <div className="contact-group-detail">
    <div className="contact-profile-top">
      <div className="contact-identity">
        <span className="contact-avatar large contact-group-avatar"><Users size={ICON_SIZE.display} /></span>
        <div>
          <input
            className="contact-name-input"
            aria-label="Group name"
            maxLength={MAX_CONTACT_GROUP_NAME}
            value={name}
            disabled={busy}
            onChange={(event) => setName(event.target.value)}
            onBlur={() => void rename()}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                event.currentTarget.blur();
              } else if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                setName(group.name);
              }
            }}
          />
          <p className="contact-identity-meta">{memberCountLabel(members.length)}</p>
        </div>
      </div>
      <div className="contact-profile-actions">
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => setAdding(true)}><UserPlus size={ICON_SIZE.sm} />Add Members</button>
        <button type="button" className="btn-icon" aria-label="Delete group" disabled={busy} onClick={() => setConfirmDelete(true)}><Trash2 size={ICON_SIZE.lg} /></button>
      </div>
    </div>
    {confirmDelete ? <InlineConfirm
      ariaLabel="Delete group"
      cancelLabel="Cancel"
      onCancel={() => setConfirmDelete(false)}
      disabled={busy}
      actions={[{ label: "Delete Group", className: "btn-danger", onClick: () => void onDelete() }]}
    >Delete “{group.name}”? Its contacts stay in your address book.</InlineConfirm> : null}
    <section className="contact-group-members" aria-label="Members">
      {members.length ? members.map((member) => <div key={member.id} className="contact-group-member">
        <button type="button" className="contact-list-item" onClick={() => onOpenContact(member.id)}>
          <ContactAvatar profile={member} />
          <span className="contact-list-copy"><strong>{contactName(member)}</strong><small>{member.addresses[0]}</small></span>
        </button>
        <button type="button" className="btn-icon btn-icon-sm" aria-label={`Remove ${contactName(member)} from ${group.name}`} disabled={busy} onClick={() => void onRemoveMember(member.id)}><X size={ICON_SIZE.sm} /></button>
      </div>) : <p className="contacts-empty">No members yet. Add contacts, or type an email address to add someone new.</p>}
    </section>
    {adding ? <ContactMemberPicker group={group} profiles={profiles} onAdd={onAddMembers} onClose={() => setAdding(false)} /> : null}
  </div>;
}

/** Finds a contact to add, or adds a typed address as a new contact. */
function ContactMemberPicker({
  group,
  profiles,
  onAdd,
  onClose,
}: {
  group: ContactGroup;
  profiles: readonly ContactProfile[];
  onAdd(contactIds: string[], emails: string[]): Promise<void>;
  onClose(): void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const listId = useId();
  const run = async (contactIds: string[], emails: string[]) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await onAdd(contactIds, emails);
      onClose();
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };
  return <Modal title={`Add to “${group.name}”`} className="goal-link-modal" onClose={onClose}>
    <FindOrCreatePicker
      items={groupCandidates(group, profiles)}
      getSearchText={(profile) => `${profile.displayName ?? ""} ${profile.addresses.join(" ")}`}
      placeholder="Find a contact, or type an email address"
      ariaLabel="Find a contact to add"
      listId={listId}
      listLabel="Contacts"
      emptyMessage="Everyone in your address book is already in this group. Type an email address to add someone new."
      createLabel={(query) => typedAddress(query) ? <>Add new contact “{typedAddress(query)}”</> : <>Type a full email address to add someone new</>}
      onSelect={(profile) => void run([profile.id], [])}
      onCreate={(query) => {
        const email = typedAddress(query);
        if (email) void run([], [email]);
        else setError("Type a full email address, such as name@example.com, to add someone new.");
      }}
      renderItem={(profile, option) => <div
        key={profile.id}
        id={option.id}
        role="option"
        aria-label={contactName(profile)}
        aria-selected={option.active}
        className={option.active ? "highlighted" : undefined}
        onMouseEnter={option.onMouseEnter}
        onClick={option.onClick}
      >
        <span className="label-option-name">{contactName(profile)}</span>
        {profile.displayName ? <small className="contact-group-option-detail">{profile.addresses[0]}</small> : null}
      </div>}
    />
    {error ? <p className="contacts-error" role="alert">{error}</p> : null}
  </Modal>;
}

/** Picks a group to add contacts to, or names a new one. */
export function ContactGroupPicker({
  title,
  groups,
  currentIds = [],
  onPick,
  onCreate,
  onClose,
}: {
  title: string;
  groups: readonly ContactGroup[];
  /** Groups that already hold every chosen contact; shown checked. */
  currentIds?: readonly string[];
  onPick(groupId: string): Promise<void>;
  onCreate(name: string): Promise<void>;
  onClose(): void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const listId = useId();
  const run = async (work: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await work();
      onClose();
    } catch (reason) {
      setError(errorMessage(reason));
      setBusy(false);
    }
  };
  return <Modal title={title} className="goal-link-modal" onClose={onClose}>
    <FindOrCreatePicker
      items={[...groups]}
      getSearchText={(group) => group.name}
      placeholder="Find a group, or name a new one"
      ariaLabel="Find or create a group"
      listId={listId}
      listLabel="Groups"
      emptyMessage="No groups yet. Type a name to create one."
      createLabel={(name) => <>Create group “{name}”</>}
      onSelect={(group) => void run(() => onPick(group.id))}
      onCreate={(name) => void run(() => onCreate(name))}
      renderItem={(group, option) => {
        const current = currentIds.includes(group.id);
        return <div
          key={group.id}
          id={option.id}
          role="option"
          aria-label={current ? `${group.name}, already added` : group.name}
          aria-selected={option.active}
          className={option.active ? "highlighted" : undefined}
          onMouseEnter={option.onMouseEnter}
          onClick={option.onClick}
        >
          <span className="label-option-name">
            {current ? <Check size={ICON_SIZE.sm} /> : <span className="label-option-check-spacer" />}
            {group.name}
          </span>
          <small className="contact-group-option-detail">{memberCountLabel(group.memberIds.length)}</small>
        </div>;
      }}
    />
    {error ? <p className="contacts-error" role="alert">{error}</p> : null}
  </Modal>;
}

/** A contact's groups on its profile; changes apply at once, like keep-in-touch. */
export function ContactGroupsSection({
  groups,
  busy,
  onOpenGroup,
  onAdd,
  onRemove,
}: {
  /** Groups this contact belongs to. */
  groups: readonly ContactGroup[];
  busy: boolean;
  onOpenGroup(id: string): void;
  onAdd(): void;
  onRemove(groupId: string): Promise<void>;
}) {
  return <section className="contact-keep-in-touch contact-groups-section" aria-label="Groups">
    <header><h2>Groups</h2>
      <button type="button" className="btn btn-sm" disabled={busy} onClick={onAdd}><Plus size={ICON_SIZE.sm} />Add to Group</button>
    </header>
    {groups.length ? <div className="contact-group-chips">
      {groups.map((group) => <span key={group.id} className="recipient-chip">
        <button type="button" className="recipient-chip-label btn-link" onClick={() => onOpenGroup(group.id)}>{group.name}</button>
        <button type="button" className="recipient-chip-remove" aria-label={`Remove from ${group.name}`} disabled={busy} onClick={() => void onRemove(group.id)}><X size={ICON_SIZE.xs} /></button>
      </span>)}
    </div> : <p className="contact-kit-status">Not in any group yet.</p>}
  </section>;
}
