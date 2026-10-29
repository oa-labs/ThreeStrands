import { createContext, useContext } from "react";

/** Which availability profile one conversation's or draft's cards use. */
export type AvailabilityProfileChoice = {
  profiles: { id: string; name: string }[];
  selectedId: string;
  /** What the choice applies to, as the help text names it. */
  scope: "conversation" | "message";
  onSelect(id: string): void;
};

export const AvailabilityProfileChoiceContext = createContext<AvailabilityProfileChoice | null>(null);

/**
 * Uses another profile's hours for this conversation or message only. Shown
 * only when there is more than one profile; it sends nothing anywhere.
 */
export function AvailabilityProfileSelect({ disabled = false }: { disabled?: boolean }) {
  const choice = useContext(AvailabilityProfileChoiceContext);
  if (!choice || choice.profiles.length < 2) return null;
  return <label className="availability-profile-select">Usual Hours
    <select
      value={choice.selectedId}
      disabled={disabled}
      title={`Applies to this ${choice.scope} until ThreeStrands quits. Set each account's profile in Settings → Availability.`}
      onChange={(event) => choice.onSelect(event.target.value)}
    >
      {choice.profiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name}</option>)}
    </select>
  </label>;
}
