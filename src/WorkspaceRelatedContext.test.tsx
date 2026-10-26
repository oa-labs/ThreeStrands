import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkspaceRelatedContext } from "./WorkspaceRelatedContext";
import { mailClient } from "./data/client";
import type { WorkspaceConversation, WorkspaceContextTarget, WorkspaceFile } from "./domain";

const meeting=(id:string,title:string):WorkspaceContextTarget=>({kind:"meeting",event:{id,title,accountId:"calendar@example.com",start:"2026-10-13T10:00:00Z",end:"2026-10-13T11:00:00Z",allDay:false,attendees:["alice@example.com"]}});
describe("automatic workspace context", () => {
  afterEach(()=>{cleanup();vi.restoreAllMocks();localStorage.clear();});
  it("ignores an old selection's late results and keeps conversations when file retrieval fails",async()=>{
    let resolveFirst!:(value:WorkspaceConversation[])=>void;
    const candidates=vi.spyOn(mailClient,"contextCandidates").mockImplementationOnce(()=>new Promise((resolve)=>{resolveFirst=resolve;})).mockResolvedValue([{threadId:"new",accountId:"work@example.com",subject:"Orion forecast",sentAt:"2026-10-01T10:00:00Z",participants:["alice@example.com"]}]);
    vi.spyOn(mailClient,"listFiles").mockRejectedValue(new Error("Files unavailable"));
    const props={accountIds:["work@example.com"],onOpenThread:vi.fn()};
    const view=render(<WorkspaceRelatedContext {...props} target={meeting("first","Atlas forecast")} />);
    await waitFor(()=>expect(candidates).toHaveBeenCalledTimes(1));
    view.rerender(<WorkspaceRelatedContext {...props} target={meeting("second","Orion forecast")} />);
    await screen.findByRole("button",{name:/Orion forecast/});
    await act(async()=>resolveFirst([{threadId:"old",accountId:"work@example.com",subject:"Atlas forecast",sentAt:"2026-10-01T10:00:00Z",participants:["alice@example.com"]}]));
    expect(screen.queryByRole("button",{name:/Atlas forecast/})).not.toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("Files unavailable");
  });
  it("counts every related file and reaches the ones past the first rows",async()=>{
    vi.spyOn(mailClient,"contextCandidates").mockResolvedValue([{threadId:"atlas",accountId:"work@example.com",subject:"Atlas forecast",sentAt:"2026-10-01T10:00:00Z",participants:["alice@example.com"]}]);
    const files:WorkspaceFile[]=Array.from({length:8},(_,index)=>({accountId:"work@example.com",messageId:`m${index}`,threadId:"atlas",subject:"Atlas forecast",sentAt:`2026-10-0${index+1}T10:00:00Z`,sender:"alice@example.com",direction:"received",attachment:{id:"part:0.1",filename:`atlas-${index}.pdf`,mimeType:"application/pdf",size:10},starred:false,availability:"unknown"}));
    vi.spyOn(mailClient,"listFiles").mockResolvedValue({files,total:files.length});
    render(<WorkspaceRelatedContext accountIds={["work@example.com"]} onOpenThread={vi.fn()} target={meeting("atlas","Atlas forecast")} />);
    const section=within(await screen.findByRole("region",{name:/Files/}));
    expect(section.getAllByRole("button",{name:/^atlas-\d\.pdf/})).toHaveLength(6);
    fireEvent.click(section.getByRole("button",{name:"Show 2 more"}));
    expect(section.getAllByRole("button",{name:/^atlas-\d\.pdf/})).toHaveLength(8);
  });
  it("names an invitation by its meeting, as Files does, and keeps the filename on its actions",async()=>{
    vi.spyOn(mailClient,"contextCandidates").mockResolvedValue([{threadId:"atlas",accountId:"work@example.com",subject:"Atlas forecast",sentAt:"2026-10-01T10:00:00Z",participants:["alice@example.com"]}]);
    const file=(id:string,subject:string,filename:string,mimeType:string):WorkspaceFile=>({accountId:"work@example.com",messageId:id,threadId:"atlas",subject,sentAt:"2026-10-01T10:00:00Z",sender:"alice@example.com",direction:"received",attachment:{id:"part:0.1",filename,mimeType,size:10},starred:false,availability:"unknown"});
    vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[file("m1","Invitation: Atlas forecast @ Tue Oct 13, 2026 10am","invite.ics","text/calendar"),file("m2","Atlas forecast deck","atlas.pdf","application/pdf")],total:2});
    render(<WorkspaceRelatedContext accountIds={["work@example.com"]} onOpenThread={vi.fn()} target={meeting("atlas","Atlas forecast")} />);
    const section=within(await screen.findByRole("region",{name:/Files/}));
    expect(section.getByRole("button",{name:/^Atlas forecastOct/})).toBeInTheDocument();
    expect(section.queryByRole("button",{name:/^invite\.ics/})).not.toBeInTheDocument();
    expect(section.getByRole("button",{name:"Star invite.ics"})).toBeInTheDocument();
    // Other files keep their filename even when the email is about the same meeting.
    expect(section.getByRole("button",{name:/^atlas\.pdf/})).toBeInTheDocument();
  });
  it("shows one invitation from a meeting series and says a shared reason once per section",async()=>{
    vi.spyOn(mailClient,"contextCandidates").mockResolvedValue(["w10","w11","deck"].map((threadId)=>({threadId,accountId:"work@example.com",subject:`Atlas sync ${threadId}`,sentAt:"2026-10-01T10:00:00Z",participants:["alice@example.com"]})));
    const file=(id:string,subject:string,filename:string,mimeType:string,sentAt:string):WorkspaceFile=>({accountId:"work@example.com",messageId:id,threadId:id,subject,sentAt,sender:"alice@example.com",direction:"received",attachment:{id:"part:0.1",filename,mimeType,size:10},starred:false,availability:"unknown"});
    vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[
      file("w10","Invitation: Atlas sync - Week 10","invite.ics","text/calendar","2026-10-12T10:00:00Z"),
      file("w11","Invitation: Atlas sync - Week 11","invite.ics","text/calendar","2026-10-05T10:00:00Z"),
      file("deck","Atlas sync deck","atlas.pdf","application/pdf","2026-10-01T10:00:00Z"),
    ],total:3});
    render(<WorkspaceRelatedContext accountIds={["work@example.com"]} onOpenThread={vi.fn()} target={meeting("w11","Atlas sync - Week 11")} />);
    const files=await screen.findByRole("region",{name:/Files/});
    expect(within(files).getAllByRole("button",{name:/^(Atlas sync|atlas\.pdf)/}).map((row)=>row.querySelector(".context-row-title")?.textContent)).toEqual(["Atlas sync - Week 11","atlas.pdf"]);
    expect(within(files).getByText("Matching topic and attendees")).toHaveClass("context-section-note");
    expect(files.querySelectorAll(".context-row")[0]).not.toHaveTextContent("Matching topic");
    const conversations=screen.getByRole("region",{name:/Conversations/});
    expect(within(conversations).getAllByText("Matching topic and attendees")).toHaveLength(1);
  });
});
