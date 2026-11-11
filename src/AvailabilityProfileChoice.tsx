import { createContext, useContext } from "react";
import type { AvailabilityPreferences, MeetingType } from "./domain";

/** Which availability profile one conversation's or draft's cards use. */
export type AvailabilityProfileChoice = {
  profiles: { id: string; name: string }[];
  selectedId: string;
  forMeetingType?(type?: MeetingType | null): { selectedId: string | null; preferences: AvailabilityPreferences };
  manual?: boolean;
  onReset?(): void;
  onOpenAvailabilitySettings?(): void;
  /** What the choice applies to, as the help text names it. */
  scope: "conversation" | "message";
  onSelect(id: string): void;
};

export const AvailabilityProfileChoiceContext = createContext<AvailabilityProfileChoice | null>(null);

/**
 * Uses another profile's hours for this conversation or message only,
 * with the selected hours visible even when only one profile exists.
 */
export function AvailabilityProfileSelect({ disabled = false, meetingType }: { disabled?: boolean; meetingType?: MeetingType | null }) {
  const choice = useContext(AvailabilityProfileChoiceContext);
  if (!choice) return null;
  const meetingHours = choice.forMeetingType?.(meetingType);
  const selectedId = meetingHours ? meetingHours.selectedId : choice.selectedId;
  return <div className="availability-hours"><label className="availability-profile-select">Hours
    <select
      value={selectedId ?? ""}
      disabled={disabled}
      title={`Applies to this ${choice.scope} until ThreeStrands quits. Set each account's profile in Settings → Availability.`}
      onChange={(event) => choice.onSelect(event.target.value)}
    >
      <option value="" disabled>Choose hours</option>
      {choice.profiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name}</option>)}
    </select>
  </label>
    {choice.manual && choice.onReset ? <button type="button" className="btn-link" disabled={disabled} onClick={choice.onReset}>Use Automatic Hours</button> : null}
  </div>;
}
