import { useId, type ReactNode } from "react";
import { MEETING_TYPES, type Account, type AvailabilityProfiles } from "./domain";
import { availabilityAccountIdentity, meetingTypeLabel } from "./AvailabilitySettings";
import { availabilityAccountKey, availabilityProfileId } from "./settings";

/**
 * Account Defaults and Meeting Types both choose a profile for something, so
 * they share one row layout: what is choosing on the left, and every profile
 * selector aligned in one column on the right.
 */
function ChoiceList({ subject, choice, children, hasActions = false }: {
  subject: string;
  choice: string;
  children: ReactNode;
  hasActions?: boolean;
}) {
  return (
    <div className={`availability-choice-list${hasActions ? " has-actions" : ""}`}>
      <div className="availability-choice-columns" aria-hidden="true">
        <span>{subject}</span>
        <span>{choice}</span>
      </div>
      {children}
    </div>
  );
}

function NoOtherProfiles({ text, onOpenProfiles }: { text: string; onOpenProfiles?(): void }) {
  return (
    <div className="settings-row">
      <span className="settings-hint">{text}</span>
      {onOpenProfiles ? <button className="btn btn-sm" type="button" onClick={onOpenProfiles}>Open Profiles</button> : null}
    </div>
  );
}

function AccountDefaultRow({ account, profiles, onProfilesChange }: {
  account: Account;
  profiles: AvailabilityProfiles;
  onProfilesChange(value: AvailabilityProfiles): void;
}) {
  const identityId = useId();
  const { name, address } = availabilityAccountIdentity(account);
  return (
    <div className="availability-choice-row">
      <span className="availability-choice-subject">
        <strong id={identityId}>{name}</strong>
        {address ? <small id={`${identityId}-address`}>{address}</small> : null}
      </span>
      <select aria-labelledby={address ? `${identityId} ${identityId}-address` : identityId} value={availabilityProfileId(profiles, account.email)} onChange={(event) => {
        const key = availabilityAccountKey(account.email);
        const { [key]: _previous, ...others } = profiles.accountDefaults;
        onProfilesChange({
          ...profiles,
          accountDefaults: event.target.value === profiles.defaultProfile.id ? others : { ...others, [key]: event.target.value },
        });
      }}>
        <option value={profiles.defaultProfile.id}>Use default · {profiles.defaultProfile.name}</option>
        {profiles.otherProfiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name}</option>)}
      </select>
    </div>
  );
}

/** Chooses the profile each mail account usually finds times with. */
export function AvailabilityAccountDefaultsSettings({ profiles, onProfilesChange, accounts, onOpenProfiles }: {
  profiles: AvailabilityProfiles;
  onProfilesChange(value: AvailabilityProfiles): void;
  accounts: Account[];
  onOpenProfiles?(): void;
}) {
  return (
    <section className="settings-section availability-settings" aria-label="Account Defaults">
      <p className="settings-hint">These hours apply unless a matching meeting type or your choice in the conversation takes precedence.</p>
      {accounts.length === 0 ? <p className="settings-hint">Connect a mail account to choose its profile.</p> : (
        <ChoiceList subject="Email account" choice="Usual profile">
          {accounts.map((account) => <AccountDefaultRow key={account.email} account={account} profiles={profiles} onProfilesChange={onProfilesChange} />)}
        </ChoiceList>
      )}
      {profiles.otherProfiles.length === 0 ? (
        <NoOtherProfiles text={`Every account uses ${profiles.defaultProfile.name}. Add a profile to give an account different hours.`} onOpenProfiles={onOpenProfiles} />
      ) : null}
    </section>
  );
}

/** Chooses the profile lunch, coffee, and other recognized invitations use. */
export function MeetingTypeProfilesSettings({ profiles, onProfilesChange, onEditProfile, onOpenProfiles }: {
  profiles: AvailabilityProfiles;
  onProfilesChange(value: AvailabilityProfiles): void;
  /** Opens the Profiles page with this profile selected. */
  onEditProfile?(id: string): void;
  onOpenProfiles?(): void;
}) {
  const idPrefix = useId();
  const allProfiles = [profiles.defaultProfile, ...profiles.otherProfiles];
  return (
    <section className="settings-section availability-settings" aria-label="Meeting Types">
      <p className="settings-hint">Find Availability uses the associated profile when it recognizes one of these meeting types. Other meetings use the account’s usual profile. Calendar conflicts and buffers still apply.</p>
      <ChoiceList subject="Meeting type" choice="Profile" hasActions={Boolean(onEditProfile)}>
        {MEETING_TYPES.map((type) => {
          const label = meetingTypeLabel(type);
          const labelId = `${idPrefix}-${type}`;
          const chosen = allProfiles.find((profile) => profile.id === profiles.meetingTypeProfiles?.[type]);
          return (
            <div className="availability-choice-row" key={type}>
              <strong className="availability-choice-subject" id={labelId}>{label}</strong>
              <select aria-labelledby={labelId} value={chosen?.id ?? ""} onChange={(event) => {
                const next = { ...profiles.meetingTypeProfiles };
                if (event.target.value) next[type] = event.target.value;
                else delete next[type];
                onProfilesChange({ ...profiles, meetingTypeProfiles: next });
              }}>
                <option value="">Choose hours when needed</option>
                {allProfiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name}</option>)}
              </select>
              {onEditProfile ? (chosen
                ? <button className="btn-link" type="button" aria-label={`Edit Profile ${chosen.name} for ${label}`} onClick={() => onEditProfile(chosen.id)}>Edit Profile</button>
                : <span />) : null}
            </div>
          );
        })}
      </ChoiceList>
      {profiles.otherProfiles.length === 0 ? (
        <NoOtherProfiles text="Add a profile, such as Lunch, to offer different hours for these invitations." onOpenProfiles={onOpenProfiles} />
      ) : null}
    </section>
  );
}
