import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ContactProfile } from "./domain";
import { ContactsWorkspace } from "./ContactsWorkspace";
import { mailClient } from "./data/client";

vi.mock("./data/client",()=>({mailClient:{listContactProfiles:vi.fn(),getContactProfile:vi.fn(),saveContactProfile:vi.fn(),deleteContactProfile:vi.fn(),contactTimeline:vi.fn(),enrichContact:vi.fn()}}));

const jane:ContactProfile={id:"contact:jane@example.com",displayName:"Jane Doe",role:"Founder",company:null,location:null,bio:null,notes:null,links:[],photoData:null,favorite:false,addresses:["jane@example.com"],sentCount:3,receivedCount:2,lastInteractedAt:"2026-09-20T00:00:00Z"};
const favoriteContact:ContactProfile={...jane,id:"contact:favorite@example.com",displayName:"Favorite Person",favorite:true,addresses:["favorite@example.com"],lastInteractedAt:"2026-09-10T00:00:00Z"};
const newerContact:ContactProfile={...jane,id:"contact:newer@example.com",displayName:"Newer Person",addresses:["newer@example.com"],lastInteractedAt:"2026-09-24T00:00:00Z"};

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

  it("groups favorites first and sorts each group by recent activity",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([jane,favoriteContact,newerContact]);
    render(<ContactsWorkspace onOpenThread={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    const favorites=screen.getByRole("region",{name:"Favorites"});
    const recent=screen.getByRole("region",{name:"Recent contacts"});
    expect(within(favorites).getAllByRole("button").map(button=>button.querySelector(".contact-list-copy strong")?.textContent)).toEqual(["Favorite Person"]);
    expect(within(recent).getAllByRole("button").map(button=>button.querySelector(".contact-list-copy strong")?.textContent)).toEqual(["Newer Person","Jane Doe"]);
    expect(screen.getByText("Favorites first · then recent activity")).toBeInTheDocument();
  });

  it("selects the contact opened from the conversation pane",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([jane,favoriteContact]);
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===favoriteContact.id?favoriteContact:jane);
    render(<ContactsWorkspace onOpenThread={vi.fn()} initialContactId={favoriteContact.id}/>);
    await screen.findByDisplayValue("Favorite Person");
    const target=screen.getByRole("button",{name:/Favorite Person/});
    expect(target).toHaveAttribute("aria-pressed","true");
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
