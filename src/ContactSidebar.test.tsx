import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ContactSidebar } from "./ContactSidebar";
import { mailClient } from "./data/client";
import type { Account, ContactProfile, ThreadDetail } from "./domain";

vi.mock("./data/client",()=>({mailClient:{listContactProfiles:vi.fn(),getContactProfile:vi.fn(),contactTimeline:vi.fn(),saveContactProfile:vi.fn()}}));

const jane:ContactProfile={id:"contact:jane@example.com",displayName:"Jane Doe",role:null,company:"Acme",location:null,bio:null,notes:null,links:[],photoData:null,favorite:false,addresses:["jane@example.com"],sentCount:1,receivedCount:1,lastInteractedAt:null};
const bob:ContactProfile={...jane,id:"contact:bob@example.com",displayName:"Bob Lee",addresses:["bob@example.com"]};
const detail={thread:{id:"thread-1"},messages:[{id:"1",sender:"Jane Doe <jane@example.com>",recipients:["You <you@example.com>","Bob Lee <bob@example.com>"],sentAt:"2026-09-24T00:00:00Z"},{id:"2",sender:"Bob Lee <bob@example.com>",recipients:["You <you@example.com>"],sentAt:"2026-09-25T00:00:00Z"}]} as unknown as ThreadDetail;
const account={email:"you@example.com"} as Account;

describe("ContactSidebar",()=>{
  afterEach(()=>{cleanup();vi.clearAllMocks();});
  it("defaults to the latest external sender and lets the reader switch participants",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockImplementation(async query=>query?.includes("jane")?[jane]:[bob]);
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    render(<ContactSidebar detail={detail} accounts={[account]} onOpenThread={vi.fn()}/>);
    await screen.findByRole("heading",{name:"Bob Lee"});
    const picker=screen.getByRole("combobox",{name:"Conversation participant"});
    fireEvent.change(picker,{target:{value:"jane@example.com"}});
    await screen.findByRole("heading",{name:"Jane Doe"});
    await waitFor(()=>expect(mailClient.listContactProfiles).toHaveBeenLastCalledWith("jane@example.com",100));
  });

  it("shows an error when saving a favorite fails",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([bob]);
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    vi.mocked(mailClient.saveContactProfile).mockRejectedValue(new Error("Address belongs to another contact"));
    render(<ContactSidebar detail={detail} accounts={[account]} onOpenThread={vi.fn()}/>);
    await screen.findByRole("heading",{name:"Bob Lee"});
    fireEvent.click(screen.getByRole("button",{name:"Add favorite"}));
    expect(await screen.findByRole("alert")).toHaveTextContent("Address belongs to another contact");
    expect(screen.getByRole("button",{name:"Add favorite"})).toBeInTheDocument();
  });

  it("copies the selected participant email from the contact pane",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([bob]);
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const writeText=vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator,"clipboard",{value:{writeText},configurable:true});
    render(<ContactSidebar detail={detail} accounts={[account]} onOpenThread={vi.fn()}/>);
    await screen.findByRole("heading",{name:"Bob Lee"});

    fireEvent.click(screen.getByRole("button",{name:"Copy email address"}));

    expect(writeText).toHaveBeenCalledWith("bob@example.com");
    expect(await screen.findByRole("button",{name:"Copied email address"})).toBeInTheDocument();
  });
});
