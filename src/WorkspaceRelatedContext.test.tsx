import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkspaceRelatedContext } from "./WorkspaceRelatedContext";
import { mailClient } from "./data/client";
import type { WorkspaceConversation, WorkspaceContextTarget } from "./domain";

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
});
