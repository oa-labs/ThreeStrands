/** Availability choices changed locally or arrived from another device. */
export const CALENDAR_AVAILABILITY_CHANGED = "calendar-availability-changed";

/** Google holiday feed IDs are independent of country and display language. */
export function isHolidayCalendar(id: string): boolean {
  return id.toLowerCase().endsWith("#holiday@group.v.calendar.google.com");
}
