import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FindTimesCard } from "./FindTimesCard";
import { ProposedTimesCard } from "./ProposedTimesCard";
import { AvailabilitySection } from "./RecipientSections";
import { CALENDAR_AVAILABILITY_CHANGED } from "./calendarAvailability";
import { mailClient } from "./data/client";
import type { AvailabilityPreferences, CheckedProposedTime, ProposedTime } from "./domain";

const preferences: AvailabilityPreferences = { timeZone: "America/New_York", workingWindows: [1,2,3,4,5].map((weekday) => ({weekday, start:"09:00",end:"17:00"})), defaultDurationMinutes:30, slotIncrementMinutes:30, driveTimeBufferMinutes:15 };
const locals = ["2026-10-31T14:00","2026-10-31T19:00","2026-11-01T16:00","2026-11-01T19:00","2026-11-02T19:00","2026-11-03T19:00","2026-11-04T19:00"];
const times: ProposedTime[] = locals.map((localStart,index) => ({ sourceText: `${index+1}) ${localStart}`, localStart, durationMinutes:60, timeZone:null }));
const row = (index: number, status = "free"): CheckedProposedTime => ({
  start: `${locals[index]}:00${index < 2 ? "-04:00" : "-05:00"}`,
  end: `${locals[index].slice(0,11)}${String(Number(locals[index].slice(11,13))+1).padStart(2,"0")}:00:00${index < 2 ? "-04:00" : "-05:00"}`,
  issue:null, outsideWorkingHours:true,
  check:{status:status as "free", conflicts:[], checkedCalendarCount:status === "unverified" ? 0 : 1, totalCalendarCount:2, uncheckedCalendars:status === "partiallyChecked" ? ["Personal"] : [], errors:[]},
});
const props = () => ({ times, preferences, calendarConnected:true, onReplyWithTimes:vi.fn(), onFindOtherTimes:vi.fn(), onOpenCalendarSettings:vi.fn() });
beforeEach(() => { vi.useFakeTimers({toFake:["Date"]}); vi.setSystemTime(new Date("2026-10-28T12:00:00Z")); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });

describe("proposed times", () => {
  it("keeps unresolved sender text inert and visible", async () => {
    const sourceText = '<img src="https://sender.example/track" onerror="alert(1)"> Saturday, time unclear';
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue([{start:null,end:null,issue:"Date or time needs clarification",check:null,outsideWorkingHours:false}]);
    const {container} = render(<ProposedTimesCard {...props()} times={[{sourceText,localStart:null,durationMinutes:60,timeZone:null}]} />);
    await screen.findByText("Date or time needs clarification");
    expect(screen.getByText(sourceText)).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });

  it("checks all seven exact hours, keeps conflicts and unusual hours visible, and drafts only selected free times", async () => {
    const rows = times.map((_,index) => row(index,index === 0 ? "conflicting" : "free"));
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(rows);
    const current = props();
    render(<ProposedTimesCard {...current} />);
    await screen.findAllByText("Free · Outside your usual hours");
    expect(check).toHaveBeenCalledWith({times, assumedTimeZone:"America/New_York", preferences});
    expect(screen.getAllByRole("checkbox")).toHaveLength(7);
    expect(screen.getByText("Conflict · Outside your usual hours")).toBeInTheDocument();
    expect(screen.getAllByRole("checkbox")[0]).toBeDisabled();
    for (const input of screen.getAllByRole("checkbox")) expect(input).not.toBeChecked();
    fireEvent.click(screen.getAllByRole("checkbox")[2]);
    fireEvent.click(screen.getAllByRole("checkbox")[6]);
    fireEvent.click(screen.getByRole("button",{name:"Draft Reply With Selected Times"}));
    await waitFor(() => expect(current.onReplyWithTimes).toHaveBeenCalledWith([2,6].map((index) => ({start:rows[index].start,end:rows[index].end,status:"verified"}))));
    expect(check).toHaveBeenCalledTimes(2);
  });

  it("preserves uncertainty and partial coverage instead of calling those times free", async () => {
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue([
      row(0,"partiallyChecked"), row(1,"unverified"),
      {...row(2),start:null,end:null,check:null,outsideWorkingHours:false,issue:"Meeting length needs clarification"},
    ]);
    render(<ProposedTimesCard {...props()} times={times.slice(0,3)} />);
    await screen.findByText(/Partially checked/);
    expect(screen.getByText(/Unable to check/)).toBeInTheDocument();
    expect(screen.getByText("Meeting length needs clarification")).toBeInTheDocument();
    expect(screen.getByText(/Couldn’t check: Personal/)).toBeInTheDocument();
    for (const input of screen.getAllByRole("checkbox")) expect(input).toBeDisabled();
  });

  it("rechecks on timezone, calendar preference, and calendar availability changes", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const current = props(); const view = render(<ProposedTimesCard {...current} />);
    await screen.findAllByText("Free · Outside your usual hours");
    fireEvent.click(screen.getAllByRole("checkbox")[1]);
    fireEvent.change(screen.getByRole("combobox",{name:"Assuming timezone"}),{target:{value:"Europe/London"}});
    await waitFor(() => expect(check).toHaveBeenCalledTimes(2));
    expect(check.mock.lastCall?.[0].assumedTimeZone).toBe("Europe/London");
    expect(screen.getAllByRole("checkbox")[1]).not.toBeChecked();
    view.rerender(<ProposedTimesCard {...current} preferences={{...preferences, driveTimeBufferMinutes:30}} />);
    await waitFor(() => expect(check).toHaveBeenCalledTimes(3));
    act(() => window.dispatchEvent(new Event(CALENDAR_AVAILABILITY_CHANGED)));
    await waitFor(() => expect(check).toHaveBeenCalledTimes(4));
  });

  it("blocks a stale claim when a selected time gains a conflict or coverage is lost", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const current = props(); render(<ProposedTimesCard {...current} />);
    await screen.findAllByText("Free · Outside your usual hours");
    fireEvent.click(screen.getAllByRole("checkbox")[1]);
    check.mockResolvedValue(times.map((_,index) => row(index,index === 1 ? "partiallyChecked" : "free")));
    fireEvent.click(screen.getByRole("button",{name:"Draft Reply With Selected Times"}));
    await screen.findByRole("alert");
    expect(current.onReplyWithTimes).not.toHaveBeenCalled();
    expect(screen.getAllByRole("checkbox")[1]).not.toBeChecked();
  });

  it("shows retryable calendar failures without discarding the sender's list", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockRejectedValueOnce(new Error("Calendar offline")).mockResolvedValue(times.map((_,index) => row(index)));
    render(<ProposedTimesCard {...props()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Calendar offline");
    expect(screen.getAllByRole("checkbox")).toHaveLength(7);
    fireEvent.click(screen.getByRole("button",{name:"Check Again"}));
    await screen.findAllByText("Free · Outside your usual hours");
    expect(check).toHaveBeenCalledTimes(2);
  });

  it("uses offered alternatives instead of range suggestions until Find Other Times is pressed", async () => {
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const search = vi.spyOn(mailClient,"findAvailability").mockResolvedValue({candidates:[],checkedCalendarCount:1,totalCalendarCount:1,uncheckedCalendars:[],errors:[]});
    render(<FindTimesCard plan={{query:null,proposedTimes:times,durationMinutes:60,timeZoneAssumed:false,bounds:{weekdays:[0,6],dayStart:"19:00",dayEnd:"20:00"}}} preferences={preferences} calendarConnected ai={null} onAddToCalendar={vi.fn()} onConfirmTime={vi.fn()} onReplyWithTimes={vi.fn()} onOpenCalendarSettings={vi.fn()} />);
    await screen.findAllByText("Free · Outside your usual hours");
    expect(search).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button",{name:"Find Other Times"}));
    await waitFor(() => expect(search).toHaveBeenCalledOnce());
    expect(search).toHaveBeenCalledWith(expect.objectContaining({preferences:expect.objectContaining({workingWindows:preferences.workingWindows,defaultDurationMinutes:60})}));
    expect(screen.queryByRole("region",{name:"Proposed times"})).not.toBeInTheDocument();
  });

  it("opens offered times supplied by a Brief without another AI request, even without a meeting proposal", async () => {
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const find = vi.spyOn(mailClient,"findTimes");
    const other = vi.spyOn(mailClient,"findAvailability").mockResolvedValue({candidates:[],checkedCalendarCount:1,totalCalendarCount:1,uncheckedCalendars:[],errors:[]});
    render(<AvailabilitySection compact offered={false} preferences={preferences} ai draft={{sourceId:"m1",subject:"CRT",body:""}} offeredTimes={{sourceMessageId:"m1",times}} onInsertTimes={vi.fn()} onOpenCalendarSettings={vi.fn()} />);
    expect(await screen.findByRole("region",{name:"Proposed times"})).toBeInTheDocument();
    expect(find).not.toHaveBeenCalled();
    expect(within(screen.getByRole("region",{name:"Proposed times"})).getAllByRole("checkbox")).toHaveLength(7);
    fireEvent.click(screen.getByRole("button",{name:"Find Other Times"}));
    await waitFor(() => expect(other).toHaveBeenCalledWith(expect.objectContaining({preferences:expect.objectContaining({defaultDurationMinutes:60})})));
  });

  it("lets Find Times extract alternatives without reusing a broader meeting search", async () => {
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    vi.spyOn(mailClient,"findTimes").mockResolvedValue({rangeStart:"2026-10-31T00:00:00-04:00",rangeEnd:"2026-11-05T00:00:00-05:00",durationMinutes:60,weekdays:[],dayStart:null,dayEnd:null,reason:"Offered options",proposedTimes:times});
    // AI request configuration is independently tested in ComposeContext.
    const { saveAiProvider } = await import("./aiSettings"); saveAiProvider("openai");
    render(<AvailabilitySection compact preferences={preferences} ai draft={{sourceId:"m1",subject:"CRT",body:""}} onInsertTimes={vi.fn()} onOpenCalendarSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button",{name:"Find Times"}));
    await screen.findByRole("region",{name:"Proposed times"});
    expect(screen.getAllByRole("checkbox")).toHaveLength(7);
    saveAiProvider("none");
  });
});
