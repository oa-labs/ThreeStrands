import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FindTimesCard } from "./FindTimesCard";
import { ProposedTimesCard } from "./ProposedTimesCard";
import { AvailabilitySection } from "./RecipientSections";
import { CALENDAR_AVAILABILITY_CHANGED } from "./calendarAvailability";
import { AvailabilityProfileChoiceContext } from "./AvailabilityProfileChoice";
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
  it("renders one number per pending option in shared context rows without overlapping checkbox controls", () => {
    vi.spyOn(mailClient,"checkProposedTimes").mockReturnValue(new Promise(() => {}));
    const sourceTexts = ["1) Saturday, Oct. 31st. - 2 PM", "(2) Saturday, Oct. 31st. - 7 PM", "3. Sunday, Nov. 1st - 4 PM", "Sunday, Nov. 1st - 7 PM", "10.31 at 7 PM"];
    const {container} = render(<ProposedTimesCard {...props()} times={sourceTexts.map((sourceText) => ({sourceText,localStart:null,durationMinutes:60,timeZone:null}))} />);
    const titles = [...container.querySelectorAll(".proposed-time-row .context-row-title")].map((title) => title.textContent);
    expect(titles).toEqual(["1. Saturday, Oct. 31st. - 2 PM", "2. Saturday, Oct. 31st. - 7 PM", "3. Sunday, Nov. 1st - 4 PM", "4. Sunday, Nov. 1st - 7 PM", "5. 10.31 at 7 PM"]);
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveClass("context-status");
    const candidates = screen.getAllByRole("button",{name:/^\d+\./});
    expect(candidates).toHaveLength(5);
    for (const candidate of candidates) {
      expect(candidate).toBeDisabled();
      expect(candidate).toHaveAttribute("aria-pressed","false");
      expect(candidate).toHaveClass("context-row-main");
    }
  });

  it("checks the offered times again against another profile's usual hours", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const onSelect = vi.fn();
    const choice = (selectedId: string) => ({ profiles:[{id:"work",name:"Work"},{id:"church",name:"Church"}], selectedId, scope:"conversation" as const, onSelect });
    const evenings = { ...preferences, workingWindows:[{weekday:6,start:"18:00",end:"22:00"}] };
    const view = render(<AvailabilityProfileChoiceContext.Provider value={choice("work")}><ProposedTimesCard {...props()} /></AvailabilityProfileChoiceContext.Provider>);
    await waitFor(() => expect(check).toHaveBeenCalledOnce());
    const profile = await screen.findByRole("combobox",{name:"Usual Hours"});
    expect(profile).toHaveAccessibleDescription();
    fireEvent.change(profile,{target:{value:"church"}});
    expect(onSelect).toHaveBeenCalledWith("church");

    view.rerender(<AvailabilityProfileChoiceContext.Provider value={choice("church")}><ProposedTimesCard {...props()} preferences={evenings} /></AvailabilityProfileChoiceContext.Provider>);
    await waitFor(() => expect(check).toHaveBeenLastCalledWith(expect.objectContaining({ preferences: evenings })));
    expect(screen.getByRole("combobox",{name:"Usual Hours"})).toHaveValue("church");
  });

  it("keeps an actionable warning for invalid offered-time metadata and reads the enabled Brief again", async () => {
    const retry = vi.fn().mockResolvedValue(undefined);
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const search = vi.spyOn(mailClient,"findAvailability");
    const current = { compact:true, offered:false, preferences, ai:false, draft:{sourceId:"m1",subject:"CRT",body:""},onInsertTimes:vi.fn(),onOpenCalendarSettings:vi.fn() };
    const view = render(<AvailabilitySection {...current} offeredTimesError="Proposed times could not be verified." onRetryOfferedTimes={retry} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Proposed times could not be verified.");
    fireEvent.click(screen.getByRole("button",{name:"Find Times"}));
    await waitFor(() => expect(retry).toHaveBeenCalledOnce());
    expect(check).not.toHaveBeenCalled();
    expect(search).not.toHaveBeenCalled();
    view.rerender(<AvailabilitySection {...current} offeredTimes={{sourceMessageId:"m1",times}} />);
    await screen.findAllByText("Free · Outside your usual hours");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    // A subsequent Brief can invalidate its optional offer; never leave its old card active.
    view.rerender(<AvailabilitySection {...current} offeredTimesError="Proposed times could not be verified." onRetryOfferedTimes={retry} />);
    await waitFor(() => expect(screen.queryByRole("region",{name:"Proposed times"})).not.toBeInTheDocument());
    expect(screen.getByRole("alert")).toBeInTheDocument();
    view.rerender(<AvailabilitySection {...current} offeredTimesError="Proposed times could not be verified." />);
    expect(screen.getByRole("button",{name:"Find Times"})).toBeDisabled();
    expect(search).not.toHaveBeenCalled();
  });

  it("keeps retry failures visible without replacing an unreadable offered list with general free times", async () => {
    const { saveAiProvider } = await import("./aiSettings"); saveAiProvider("openai");
    const search = vi.spyOn(mailClient,"findAvailability");
    vi.spyOn(mailClient,"findTimes").mockRejectedValue(new Error("Unverifiable offered times"));
    render(<AvailabilitySection compact offered={false} preferences={preferences} ai draft={{sourceId:"m1",subject:"CRT",body:""}} offeredTimesError="Could not verify proposed times." onInsertTimes={vi.fn()} onOpenCalendarSettings={vi.fn()} />);
    fireEvent.click(screen.getByRole("button",{name:"Find Times"}));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("No proposed times were checked."));
    expect(search).not.toHaveBeenCalled();
    expect(screen.getByRole("button",{name:"Find Times"})).toBeEnabled();
    saveAiProvider("none");
  });

  it("keeps unresolved sender text inert and visible", async () => {
    const sourceText = '<img src="https://sender.example/track" onerror="alert(1)"> Saturday, time unclear';
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue([{start:null,end:null,issue:"Date or time needs clarification",check:null,outsideWorkingHours:false}]);
    const {container} = render(<ProposedTimesCard {...props()} times={[{sourceText,localStart:null,durationMinutes:60,timeZone:null}]} />);
    await screen.findByText("Date or time needs clarification");
    expect(screen.getByText(`1. ${sourceText}`)).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });

  it("checks all seven exact hours, keeps conflicts and unusual hours visible, and drafts only selected free times", async () => {
    const rows = times.map((_,index) => row(index,index === 0 ? "conflicting" : "free"));
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(rows);
    const current = props();
    render(<ProposedTimesCard {...current} />);
    await screen.findAllByText("Free · Outside your usual hours");
    expect(check).toHaveBeenCalledWith({times, assumedTimeZone:"America/New_York", preferences});
    expect(screen.getAllByRole("button", {name:/^\d+\./})).toHaveLength(7);
    expect(screen.getByText("Conflict · Outside your usual hours")).toBeInTheDocument();
    expect(screen.getAllByRole("button", {name:/^\d+\./})[0]).toBeDisabled();
    for (const input of screen.getAllByRole("button", {name:/^\d+\./})) expect(input).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(screen.getAllByRole("button", {name:/^\d+\./})[2]);
    fireEvent.click(screen.getAllByRole("button", {name:/^\d+\./})[6]);
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
    for (const input of screen.getAllByRole("button", {name:/^\d+\./})) expect(input).toBeDisabled();
  });

  it("rechecks on timezone, calendar preference, and calendar availability changes", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const current = props(); const view = render(<ProposedTimesCard {...current} />);
    await screen.findAllByText("Free · Outside your usual hours");
    fireEvent.click(screen.getAllByRole("button", {name:/^\d+\./})[1]);
    fireEvent.change(screen.getByRole("combobox",{name:"Assuming timezone"}),{target:{value:"Europe/London"}});
    await waitFor(() => expect(check).toHaveBeenCalledTimes(2));
    expect(check.mock.lastCall?.[0].assumedTimeZone).toBe("Europe/London");
    expect(screen.getAllByRole("button", {name:/^\d+\./})[1]).toHaveAttribute("aria-pressed", "false");
    view.rerender(<ProposedTimesCard {...current} preferences={{...preferences, driveTimeBufferMinutes:30}} />);
    await waitFor(() => expect(check).toHaveBeenCalledTimes(3));
    act(() => window.dispatchEvent(new Event(CALENDAR_AVAILABILITY_CHANGED)));
    await waitFor(() => expect(check).toHaveBeenCalledTimes(4));
  });

  it("blocks a stale claim when a selected time gains a conflict or coverage is lost", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const current = props(); render(<ProposedTimesCard {...current} />);
    await screen.findAllByText("Free · Outside your usual hours");
    fireEvent.click(screen.getAllByRole("button", {name:/^\d+\./})[1]);
    check.mockResolvedValue(times.map((_,index) => row(index,index === 1 ? "partiallyChecked" : "free")));
    fireEvent.click(screen.getByRole("button",{name:"Draft Reply With Selected Times"}));
    await screen.findByRole("alert");
    expect(current.onReplyWithTimes).not.toHaveBeenCalled();
    expect(screen.getAllByRole("button", {name:/^\d+\./})[1]).toHaveAttribute("aria-pressed", "false");
  });

  it("shows retryable calendar failures without discarding the sender's list", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockRejectedValueOnce(new Error("Calendar offline")).mockResolvedValue(times.map((_,index) => row(index)));
    render(<ProposedTimesCard {...props()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Calendar offline");
    expect(screen.getAllByRole("button", {name:/^\d+\./})).toHaveLength(7);
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
    expect(within(screen.getByRole("region",{name:"Proposed times"})).getAllByRole("button", {name:/^\d+\./})).toHaveLength(7);
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
    expect(screen.getAllByRole("button", {name:/^\d+\./})).toHaveLength(7);
    saveAiProvider("none");
  });
});
