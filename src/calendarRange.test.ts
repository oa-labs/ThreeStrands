import { afterEach, describe, expect, it } from "vitest";
import { calendarRange, readCalendarRange } from "./calendarRange";

describe("calendar date ranges", () => {
  afterEach(()=>localStorage.clear());
  it("uses local calendar days across daylight saving changes", () => {
    const day=new Date(2026,10,1);
    const range=calendarRange(day,"day");
    expect(range.start).toEqual(new Date(2026,10,1)); expect(range.end).toEqual(new Date(2026,10,2));
    expect(range.end.getDate()).toBe(2);
    expect(calendarRange(day,"threeDays").days.map((date)=>date.getDate())).toEqual([1,2,3]);
  });
  it("makes the following workweek explicit on weekends and preserves Sunday-anchored full weeks", () => {
    for (const date of [new Date(2026,9,17),new Date(2026,9,18)]) {
      expect(calendarRange(date,"workweek").start).toEqual(new Date(2026,9,19));
      expect(calendarRange(date,"workweek").days).toHaveLength(5);
    }
    expect(calendarRange(new Date(2026,9,14),"week").start).toEqual(new Date(2026,9,11));
    expect(calendarRange(new Date(2026,9,14),"workweek").step).toBe(7);
  });
  it("defaults unreadable preferences to Workweek",()=>{
    localStorage.setItem("threestrands.calendar.range","garbage"); expect(readCalendarRange()).toBe("workweek");
  });
});
