import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ContactProfile } from "./domain";
import { ContactsWorkspace } from "./ContactsWorkspace";
import { mailClient } from "./data/client";

vi.mock("./data/client",()=>({mailClient:{listContactProfiles:vi.fn(),getContactProfile:vi.fn(),saveContactProfile:vi.fn(),deleteContactProfile:vi.fn(),contactTimeline:vi.fn(),enrichContact:vi.fn()}}));

const jane:ContactProfile={id:"contact:jane@example.com",displayName:"Jane Doe",role:"Founder",company:null,location:null,bio:null,notes:null,links:[],photoData:null,favorite:false,addresses:["jane@example.com"],sentCount:3,receivedCount:2,lastInteractedAt:"2026-09-20T00:00:00Z"};

describe("ContactsWorkspace",()=>{
  beforeEach(()=>{
    localStorage.clear();
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([jane]);
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(jane);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    vi.mocked(mailClient.saveContactProfile).mockImplementation(async request=>({...jane,...request,id:request.id??jane.id,sentCount:3,receivedCount:2,lastInteractedAt:jane.lastInteractedAt}));
    vi.mocked(mailClient.deleteContactProfile).mockResolvedValue(undefined);
    vi.mocked(mailClient.enrichContact).mockResolvedValue([]);
  });
  afterEach(()=>{cleanup();vi.clearAllMocks();localStorage.clear();});

  it("searches sent-to and saved contacts, then saves edited details",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.keyDown(window,{key:"/"});
    const search=screen.getByRole("textbox",{name:"Search contacts"});
    expect(document.activeElement).toBe(search);
    fireEvent.change(search,{target:{value:"jane"}});
    await waitFor(()=>expect(mailClient.listContactProfiles).toHaveBeenLastCalledWith("jane",500));
    fireEvent.change(screen.getByLabelText("Company"),{target:{value:"Acme"}});
    fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({company:"Acme",addresses:["jane@example.com"]})));
  });

  it("requires explicit use of each AI suggestion before saving it",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    vi.mocked(mailClient.enrichContact).mockResolvedValue([{field:"company",value:"Acme",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I work at Acme."}]);
    render(<ContactsWorkspace onOpenThread={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("I work at Acme.");
    expect(mailClient.saveContactProfile).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button",{name:"Use suggestion"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({company:"Acme"})));
  });

  it("confirms deletion of a saved profile",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:"Delete contact"}));
    fireEvent.click(screen.getByRole("button",{name:"Confirm"}));
    await waitFor(()=>expect(mailClient.deleteContactProfile).toHaveBeenCalledWith(jane.id));
  });
});
