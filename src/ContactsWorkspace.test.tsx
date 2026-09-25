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
    vi.mocked(mailClient.enrichContact).mockResolvedValue({suggestions:[],messagesReviewed:0,hasMore:false});
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
    vi.mocked(mailClient.enrichContact).mockResolvedValue({suggestions:[{field:"company",value:"Acme",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I work at Acme."}],messagesReviewed:3,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("I work at Acme.");
    expect(mailClient.saveContactProfile).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button",{name:"Use suggestion"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({company:"Acme"})));
    expect(screen.getByRole("button",{name:/Jane Doe/})).toHaveAttribute("aria-pressed","true");
  });

  it("offers more emails only after the first three produced suggestions",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    vi.mocked(mailClient.enrichContact)
      .mockResolvedValueOnce({suggestions:[{field:"company",value:"Acme",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I work at Acme."}],messagesReviewed:3,hasMore:true})
      .mockResolvedValueOnce({suggestions:[{field:"location",value:"Boston",sourceMessageId:"m2",sourceThreadId:"thread-2",excerpt:"I live in Boston."}],messagesReviewed:9,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");

    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("I work at Acme.");
    expect(screen.getByText("3 emails reviewed")).toBeInTheDocument();
    expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o",null,false);

    fireEvent.click(screen.getByRole("button",{name:"Search more emails"}));
    await screen.findByText("I live in Boston.");
    expect(screen.getByText("12 emails reviewed")).toBeInTheDocument();
    expect(screen.getByText("I work at Acme.")).toBeInTheDocument();
    expect(mailClient.enrichContact).toHaveBeenLastCalledWith(jane.id,"openai","gpt-4o",null,true);
    expect(screen.queryByRole("button",{name:"Search more emails"})).not.toBeInTheDocument();
    expect(mailClient.saveContactProfile).not.toHaveBeenCalled();
  });

  it("stays on a mail-derived person when accepting suggestions saves their profile",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    const derived={...jane,id:"derived:jane@example.com"};
    const saved={...jane,id:"contact:saved-jane",company:"Acme"};
    let didSave=false;
    vi.mocked(mailClient.listContactProfiles).mockImplementation(async()=>didSave?[favoriteContact,saved]:[favoriteContact,derived]);
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===saved.id?saved:derived);
    vi.mocked(mailClient.saveContactProfile).mockImplementation(async request=>{didSave=true;return {...saved,...request,id:saved.id};});
    vi.mocked(mailClient.enrichContact).mockResolvedValue({suggestions:[
      {field:"company",value:"Acme",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I work at Acme."},
      {field:"location",value:"Boston",sourceMessageId:"m2",sourceThreadId:"thread-2",excerpt:"I live in Boston."},
    ],messagesReviewed:3,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()} initialContactId={derived.id}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("I live in Boston.");

    fireEvent.click(screen.getAllByRole("button",{name:"Use suggestion"})[0]);

    await waitFor(()=>expect(screen.getByRole("button",{name:/Jane Doe/})).toHaveAttribute("aria-pressed","true"));
    await waitFor(()=>expect(mailClient.getContactProfile).toHaveBeenCalledWith(saved.id));
    expect(screen.getByDisplayValue("Jane Doe")).toBeInTheDocument();
    expect(screen.getByLabelText("Company")).toHaveValue("Acme");
    expect(screen.getByText("I live in Boston.")).toBeInTheDocument();
    expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({id:derived.id,company:"Acme"}));

    fireEvent.click(screen.getByRole("button",{name:"Use suggestion"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({id:saved.id,company:"Acme",location:"Boston"})));
    expect(screen.getByDisplayValue("Jane Doe")).toBeInTheDocument();
  });

  it("keeps an edited contact open when a suggestion removes them from the search results",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    let applied=false;
    vi.mocked(mailClient.listContactProfiles).mockImplementation(async()=>applied?[favoriteContact]:[jane,favoriteContact]);
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===favoriteContact.id?favoriteContact:jane);
    vi.mocked(mailClient.saveContactProfile).mockImplementation(async request=>{applied=true;return {...jane,...request,id:jane.id};});
    vi.mocked(mailClient.enrichContact).mockResolvedValue({suggestions:[{field:"role",value:"CEO",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I am the CEO."}],messagesReviewed:3,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.change(screen.getByRole("textbox",{name:"Search contacts"}),{target:{value:"Founder"}});
    await waitFor(()=>expect(mailClient.listContactProfiles).toHaveBeenLastCalledWith("Founder",500));
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("I am the CEO.");

    fireEvent.click(screen.getByRole("button",{name:"Use suggestion"}));

    await waitFor(()=>expect(screen.getByDisplayValue("CEO")).toBeInTheDocument());
    expect(screen.getByDisplayValue("Jane Doe")).toBeInTheDocument();
    expect(screen.getByRole("textbox",{name:"Search contacts"})).toHaveValue("Founder");
  });

  it("confirms deletion of a saved profile",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:"Delete contact"}));
    fireEvent.click(screen.getByRole("button",{name:"Confirm"}));
    await waitFor(()=>expect(mailClient.deleteContactProfile).toHaveBeenCalledWith(jane.id));
  });
});
