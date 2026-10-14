import { describe, expect, it } from "vitest";
import { invitationTitle } from "./invitationTitle";

describe("invitationTitle", () => {
  it("removes status words and the trailing schedule", () => {
    expect(invitationTitle("Invitation: Weekly sync @ Tue Oct 14, 2026 10am - 11am (EDT) (you@example.com)")).toBe("Weekly sync");
    expect(invitationTitle("Updated invitation with note: Budget review @ Mon Oct 20, 2026 2pm - 3pm (EDT) (you@example.com)")).toBe("Budget review");
    expect(invitationTitle("Accepted: Design crit @ Fri Oct 17, 2026 9am")).toBe("Design crit");
    expect(invitationTitle("Canceled event: Offsite")).toBe("Offsite");
    expect(invitationTitle("RE: Declined: Planning")).toBe("Planning");
  });

  it("keeps subjects that are already just the title", () => {
    expect(invitationTitle("Quarterly planning")).toBe("Quarterly planning");
    expect(invitationTitle("Lunch with alice@example.com")).toBe("Lunch with alice@example.com");
    expect(invitationTitle("Drinks @ the pub")).toBe("Drinks @ the pub");
    expect(invitationTitle("Invitations to send")).toBe("Invitations to send");
  });

  it("returns null when no title is left", () => {
    expect(invitationTitle("")).toBeNull();
    expect(invitationTitle("Invitation: ")).toBeNull();
    expect(invitationTitle("  ")).toBeNull();
  });
});
