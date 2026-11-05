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
  it("renders one number per pending option as a checkbox that waits for the calendar check", () => {
    vi.spyOn(mailClient,"checkProposedTimes").mockReturnValue(new Promise(() => {}));
    const sourceTexts = ["1) Saturday, Oct. 31st. - 2 PM", "(2) Saturday, Oct. 31st. - 7 PM", "3. Sunday, Nov. 1st - 4 PM", "Sunday, Nov. 1st - 7 PM", "10.31 at 7 PM"];
    const {container} = render(<ProposedTimesCard {...props()} times={sourceTexts.map((sourceText) => ({sourceText,localStart:null,durationMinutes:60,timeZone:null}))} />);
    const titles = [...container.querySelectorAll(".proposed-time-title")].map((title) => title.textContent);
    expect(titles).toEqual(["1. Saturday, Oct. 31st. - 2 PM", "2. Saturday, Oct. 31st. - 7 PM", "3. Sunday, Nov. 1st - 4 PM", "4. Sunday, Nov. 1st - 7 PM", "5. 10.31 at 7 PM"]);
    expect(screen.getByRole("status")).toHaveClass("context-status");
    const candidates = screen.getAllByRole("checkbox",{name:/^\d+\./});
    expect(candidates).toHaveLength(5);
    for (const candidate of candidates) {
      expect(candidate).toBeDisabled();
      expect(candidate).not.toBeChecked();
      expect(candidate).toHaveAccessibleDescription("Not checked");
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
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue([{start:null,end:null,issue:"Couldn’t tell the date or time; ask the sender in your reply",check:null,outsideWorkingHours:false}]);
    const {container} = render(<ProposedTimesCard {...props()} times={[{sourceText,localStart:null,durationMinutes:60,timeZone:null}]} />);
    await screen.findByText("Couldn’t tell the date or time; ask the sender in your reply");
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
    expect(screen.getAllByRole("checkbox", {name:/^\d+\./})).toHaveLength(7);
    expect(screen.getByText("Conflict · Outside your usual hours")).toBeInTheDocument();
    expect(screen.getAllByRole("checkbox", {name:/^\d+\./})[0]).toBeDisabled();
    for (const input of screen.getAllByRole("checkbox", {name:/^\d+\./})) expect(input).not.toBeChecked();
    fireEvent.click(screen.getAllByRole("checkbox", {name:/^\d+\./})[2]);
    fireEvent.click(screen.getAllByRole("checkbox", {name:/^\d+\./})[6]);
    fireEvent.click(screen.getByRole("button",{name:"Draft Reply With Selected Times"}));
    await waitFor(() => expect(current.onReplyWithTimes).toHaveBeenCalledWith([2,6].map((index) => ({start:rows[index].start,end:rows[index].end,status:"verified"}))));
    expect(check).toHaveBeenCalledTimes(2);
  });

  it("preserves uncertainty and partial coverage instead of calling those times free", async () => {
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue([
      row(0,"partiallyChecked"), row(1,"unverified"),
      {...row(2),start:null,end:null,check:null,outsideWorkingHours:false,issue:"This proposed time has passed"},
    ]);
    render(<ProposedTimesCard {...props()} times={times.slice(0,3)} />);
    await screen.findByText(/Partially checked/);
    expect(screen.getByText(/Unable to check/)).toBeInTheDocument();
    expect(screen.getByText("This proposed time has passed")).toBeInTheDocument();
    expect(screen.getByText(/Couldn’t check: Personal/)).toBeInTheDocument();
    for (const input of screen.getAllByRole("checkbox", {name:/^\d+\./})) expect(input).toBeDisabled();
  });

  it("assumes the usual meeting length when the email doesn't say, and rechecks when it changes", async () => {
    const unstated = [{sourceText:"Monday at 2:00 or 3:00 pm",localStart:"2026-11-02T14:00",durationMinutes:null,timeZone:null},{sourceText:"Monday at 2:00 or 3:00 pm",localStart:"2026-11-02T15:00",durationMinutes:null,timeZone:null}];
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue([row(4),row(5)]);
    render(<ProposedTimesCard {...props()} times={unstated} />);
    await waitFor(() => expect(check).toHaveBeenCalledOnce());
    expect(check.mock.lastCall?.[0].times.map((time) => time.durationMinutes)).toEqual([30,30]);
    const length = screen.getByRole("combobox",{name:"Meeting length"});
    expect(length).toHaveValue("30");
    expect(length).toHaveAccessibleDescription();
    expect(await screen.findAllByText("Free · Outside your usual hours · Assumes 30 min")).toHaveLength(2);
    // The assumption makes the times selectable instead of blocking them.
    for (const option of screen.getAllByRole("checkbox",{name:/^\d+\./})) expect(option).toBeEnabled();
    fireEvent.click(screen.getAllByRole("checkbox",{name:/^\d+\./})[0]);
    fireEvent.change(length,{target:{value:"60"}});
    await waitFor(() => expect(check).toHaveBeenCalledTimes(2));
    expect(check.mock.lastCall?.[0].times.map((time) => time.durationMinutes)).toEqual([60,60]);
    expect(screen.getAllByRole("checkbox",{name:/^\d+\./})[0]).not.toBeChecked();
  });

  it("keeps a stated meeting length and offers no length choice", async () => {
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    render(<ProposedTimesCard {...props()} />);
    await screen.findAllByText("Free · Outside your usual hours");
    expect(screen.queryByRole("combobox",{name:"Meeting length"})).not.toBeInTheDocument();
    expect(screen.getByText("7 proposed times · 1 hr each")).toBeInTheDocument();
  });

  it("names timezones the way people say them and mentions the reader's zone only when it differs", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    render(<ProposedTimesCard {...props()} />);
    await screen.findAllByText("Free · Outside your usual hours");
    const zone = screen.getByRole("combobox",{name:"Sender’s timezone"});
    expect(zone).toHaveValue("America/New_York");
    expect(within(zone).getByRole("option",{selected:true})).toHaveTextContent("Eastern Time (ET)");
    expect(within(zone).getByRole("group",{name:"Common"})).toBeInTheDocument();
    expect(within(zone).getByRole("option",{name:"Pacific Time (PT)"})).toHaveValue("America/Los_Angeles");
    expect(screen.queryByText(/Shown in your timezone/)).not.toBeInTheDocument();
    expect(screen.getByText("Check the times you want to offer.")).toBeInTheDocument();
    fireEvent.change(zone,{target:{value:"America/Los_Angeles"}});
    await waitFor(() => expect(check).toHaveBeenCalledTimes(2));
    expect(check.mock.lastCall?.[0].assumedTimeZone).toBe("America/Los_Angeles");
    expect(screen.getByText("Shown in your timezone, Eastern Time (ET).")).toBeInTheDocument();
  });

  it("asks for a choice only when one of the times can be offered", async () => {
    vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue([row(0,"conflicting")]);
    render(<ProposedTimesCard {...props()} times={times.slice(0,1)} />);
    await screen.findByText("Conflict · Outside your usual hours");
    expect(screen.queryByText("Check the times you want to offer.")).not.toBeInTheDocument();
    expect(screen.getByRole("button",{name:"Draft Reply With Selected Times"})).toBeDisabled();
  });

  it("rechecks on timezone, calendar preference, and calendar availability changes", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const current = props(); const view = render(<ProposedTimesCard {...current} />);
    await screen.findAllByText("Free · Outside your usual hours");
    fireEvent.click(screen.getAllByRole("checkbox", {name:/^\d+\./})[1]);
    fireEvent.change(screen.getByRole("combobox",{name:"Sender’s timezone"}),{target:{value:"Europe/London"}});
    await waitFor(() => expect(check).toHaveBeenCalledTimes(2));
    expect(check.mock.lastCall?.[0].assumedTimeZone).toBe("Europe/London");
    expect(screen.getAllByRole("checkbox", {name:/^\d+\./})[1]).not.toBeChecked();
    view.rerender(<ProposedTimesCard {...current} preferences={{...preferences, driveTimeBufferMinutes:30}} />);
    await waitFor(() => expect(check).toHaveBeenCalledTimes(3));
    act(() => window.dispatchEvent(new Event(CALENDAR_AVAILABILITY_CHANGED)));
    await waitFor(() => expect(check).toHaveBeenCalledTimes(4));
  });

  it("offers Accept & Add to Calendar for exactly one chosen time, rechecking it first", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const onAcceptTime = vi.fn();
    const current = props();
    render(<ProposedTimesCard {...current} onAcceptTime={onAcceptTime} />);
    await screen.findAllByText("Free · Outside your usual hours");
    expect(screen.queryByRole("button",{name:"Accept & Add to Calendar"})).not.toBeInTheDocument();
    const options = screen.getAllByRole("checkbox",{name:/^\d+\./});
    fireEvent.click(options[2]);
    const accept = screen.getByRole("button",{name:"Accept & Add to Calendar"});
    expect(accept).toHaveClass("btn-primary");
    expect(screen.getByRole("button",{name:"Draft Reply With Selected Times"})).not.toHaveClass("btn-primary");
    // Two times are an offer back, not an acceptance.
    fireEvent.click(options[4]);
    expect(screen.queryByRole("button",{name:"Accept & Add to Calendar"})).not.toBeInTheDocument();
    fireEvent.click(options[4]);
    fireEvent.click(screen.getByRole("button",{name:"Accept & Add to Calendar"}));
    await waitFor(() => expect(onAcceptTime).toHaveBeenCalledWith({start:row(2).start,end:row(2).end}));
    expect(check).toHaveBeenCalledTimes(2);
    expect(current.onReplyWithTimes).not.toHaveBeenCalled();
  });

  it("does not accept a time that stopped being free, and never offers Accept while writing a draft", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const onAcceptTime = vi.fn();
    const view = render(<ProposedTimesCard {...props()} onAcceptTime={onAcceptTime} />);
    await screen.findAllByText("Free · Outside your usual hours");
    fireEvent.click(screen.getAllByRole("checkbox",{name:/^\d+\./})[1]);
    check.mockResolvedValue(times.map((_,index) => row(index,index === 1 ? "conflicting" : "free")));
    fireEvent.click(screen.getByRole("button",{name:"Accept & Add to Calendar"}));
    await screen.findByRole("alert");
    expect(onAcceptTime).not.toHaveBeenCalled();
    view.rerender(<ProposedTimesCard {...props()} intoDraft onAcceptTime={onAcceptTime} />);
    fireEvent.click(screen.getAllByRole("checkbox",{name:/^\d+\./})[2]);
    expect(screen.queryByRole("button",{name:"Accept & Add to Calendar"})).not.toBeInTheDocument();
    expect(screen.getByRole("button",{name:"Insert Selected Times"})).toHaveClass("btn-primary");
  });

  it("blocks a stale claim when a selected time gains a conflict or coverage is lost", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockResolvedValue(times.map((_,index) => row(index)));
    const current = props(); render(<ProposedTimesCard {...current} />);
    await screen.findAllByText("Free · Outside your usual hours");
    fireEvent.click(screen.getAllByRole("checkbox", {name:/^\d+\./})[1]);
    check.mockResolvedValue(times.map((_,index) => row(index,index === 1 ? "partiallyChecked" : "free")));
    fireEvent.click(screen.getByRole("button",{name:"Draft Reply With Selected Times"}));
    await screen.findByRole("alert");
    expect(current.onReplyWithTimes).not.toHaveBeenCalled();
    expect(screen.getAllByRole("checkbox", {name:/^\d+\./})[1]).not.toBeChecked();
  });

  it("shows retryable calendar failures without discarding the sender's list", async () => {
    const check = vi.spyOn(mailClient,"checkProposedTimes").mockRejectedValueOnce(new Error("Calendar offline")).mockResolvedValue(times.map((_,index) => row(index)));
    render(<ProposedTimesCard {...props()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Calendar offline");
    expect(screen.getAllByRole("checkbox", {name:/^\d+\./})).toHaveLength(7);
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
    expect(within(screen.getByRole("region",{name:"Proposed times"})).getAllByRole("checkbox", {name:/^\d+\./})).toHaveLength(7);
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
    expect(screen.getAllByRole("checkbox", {name:/^\d+\./})).toHaveLength(7);
    saveAiProvider("none");
  });
});
