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
    const onSaved=vi.fn();
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={onSaved}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.keyDown(window,{key:"/"});
    const search=screen.getByRole("textbox",{name:"Search contacts"});
    expect(document.activeElement).toBe(search);
    fireEvent.change(search,{target:{value:"jane"}});
    await waitFor(()=>expect(mailClient.listContactProfiles).toHaveBeenLastCalledWith("jane",500,undefined));
    fireEvent.change(screen.getByLabelText("Company"),{target:{value:"Acme"}});
    fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({company:"Acme",addresses:["jane@example.com"]})));
    await waitFor(()=>expect(onSaved).toHaveBeenCalledOnce());
  });

  it("edits email addresses as removable badges",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    const input=screen.getByRole("textbox",{name:"Email addresses"});
    expect(screen.getByRole("button",{name:"Remove jane@example.com"})).toBeInTheDocument();
    fireEvent.change(input,{target:{value:"jane@work.example.com"}});
    fireEvent.keyDown(input,{key:"Enter"});
    expect(input).toHaveValue("");
    expect(screen.getByRole("button",{name:"Remove jane@work.example.com"})).toBeInTheDocument();
    fireEvent.change(input,{target:{value:"JANE@example.com"}});
    fireEvent.keyDown(input,{key:","});
    expect(screen.getAllByRole("button",{name:/^Remove /})).toHaveLength(2);
    fireEvent.paste(input,{clipboardData:{getData:()=>"one@example.com, two@example.com"}});
    fireEvent.change(input,{target:{value:"typed@example.com"}});
    fireEvent.blur(input);
    fireEvent.click(screen.getByRole("button",{name:"Remove jane@example.com"}));
    fireEvent.keyDown(input,{key:"Backspace"});
    fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({addresses:["jane@work.example.com","one@example.com","two@example.com"]})));
  });

  it("copies an address from its badge and reports clipboard failures",async()=>{
    const writeText=vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("denied"));
    Object.defineProperty(navigator,"clipboard",{value:{writeText},configurable:true});
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:"Copy jane@example.com"}));
    expect(await screen.findByRole("button",{name:"Copied jane@example.com"})).toBeInTheDocument();
    expect(writeText).toHaveBeenCalledWith("jane@example.com");
    fireEvent.click(screen.getByRole("button",{name:"Copied jane@example.com"}));
    expect(await screen.findByRole("status")).toHaveTextContent("Could not copy email address");
    expect(screen.getByRole("button",{name:"Copy jane@example.com"})).toBeInTheDocument();
  });

  it("shows the selected account and scopes contacts, history, and enrichment",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    const onOpenThread=vi.fn();
    vi.mocked(mailClient.getContactProfile).mockResolvedValue({...jane,sentCount:20,receivedCount:20});
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([{threadId:"work-thread",accountId:"work@example.com",contactEmail:"jane@example.com",subject:"Project",snippet:"",sentAt:"2026-09-20T00:00:00Z",labels:[]}]);
    render(<ContactsWorkspace accountId="work@example.com" onOpenThread={onOpenThread} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    expect(screen.getByText("· work@example.com")).toBeInTheDocument();
    expect(mailClient.listContactProfiles).toHaveBeenCalledWith("",500,"work@example.com");
    expect(mailClient.contactTimeline).toHaveBeenCalledWith(jane.id,0,20,"work@example.com");
    expect(screen.getByRole("button",{name:/Project/})).toHaveTextContent("jane@example.com");
    expect(screen.getByText("3 sent · 2 received")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:/Project/}));
    expect(onOpenThread).toHaveBeenCalledWith("work-thread");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await waitFor(()=>expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o",null,["company","location","bio","link"],false,"work@example.com"));
  });

  it("does not confirm a contact save that failed",async()=>{
    const onSaved=vi.fn();
    vi.mocked(mailClient.saveContactProfile).mockRejectedValueOnce(new Error("Could not save contact"));
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={onSaved}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save contact");
    expect(onSaved).not.toHaveBeenCalled();
  });

  it("groups favorites first and sorts each group by recent activity",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([jane,favoriteContact,newerContact]);
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    const favorites=screen.getByRole("region",{name:"Favorites"});
    const recent=screen.getByRole("region",{name:"Recent contacts"});
    expect(within(favorites).getAllByRole("button").map(button=>button.querySelector(".contact-list-copy strong")?.textContent)).toEqual(["Favorite Person"]);
    expect(within(recent).getAllByRole("button").map(button=>button.querySelector(".contact-list-copy strong")?.textContent)).toEqual(["Newer Person","Jane Doe"]);
    expect(screen.getByText("Favorites first · then recent activity")).toBeInTheDocument();
  });

  it("saves favorite changes immediately without saving other edits",async()=>{
    const onSaved=vi.fn();
    vi.mocked(mailClient.saveContactProfile).mockImplementation(async request=>({...jane,favorite:request.favorite}));
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={onSaved}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.change(screen.getByLabelText("Company"),{target:{value:"Unsaved Company"}});

    fireEvent.click(screen.getByRole("button",{name:"Favorite contact"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({id:jane.id,favorite:true,company:null})));
    await waitFor(()=>expect(screen.getByRole("button",{name:"Favorite contact"})).toHaveAttribute("aria-pressed","true"));
    expect(screen.getByLabelText("Company")).toHaveValue("Unsaved Company");
    expect(within(screen.getByRole("region",{name:"Favorites"})).getByRole("button",{name:/Jane Doe/})).toBeInTheDocument();
    expect(onSaved).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button",{name:"Favorite contact"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenLastCalledWith(expect.objectContaining({id:jane.id,favorite:false,company:null})));
    await waitFor(()=>expect(screen.getByRole("button",{name:"Favorite contact"})).toHaveAttribute("aria-pressed","false"));
    expect(screen.getByLabelText("Company")).toHaveValue("Unsaved Company");
  });

  it("keeps an unsaved edit when favoriting a mail-derived person creates their profile",async()=>{
    const derived={...jane,id:"derived:jane@example.com"};
    let saved:ContactProfile|null=null;
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([derived]);
    vi.mocked(mailClient.getContactProfile).mockImplementation(async()=>saved??derived);
    vi.mocked(mailClient.saveContactProfile).mockImplementation(async request=>{
      saved={...derived,id:"contact:saved-jane",favorite:request.favorite};
      return saved;
    });
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.change(screen.getByLabelText("Company"),{target:{value:"Unsaved Company"}});

    fireEvent.click(screen.getByRole("button",{name:"Favorite contact"}));

    await waitFor(()=>expect(mailClient.getContactProfile).toHaveBeenCalledWith("contact:saved-jane"));
    expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({id:derived.id,favorite:true,company:null}));
    expect(screen.getByLabelText("Company")).toHaveValue("Unsaved Company");
    expect(screen.getByRole("button",{name:"Favorite contact"})).toHaveAttribute("aria-pressed","true");
  });

  it("leaves the favorite unchanged when its immediate save fails",async()=>{
    vi.mocked(mailClient.saveContactProfile).mockRejectedValueOnce(new Error("Could not save favorite"));
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:"Favorite contact"}));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save favorite");
    expect(screen.getByRole("button",{name:"Favorite contact"})).toHaveAttribute("aria-pressed","false");
  });

  it("selects the contact opened from the conversation pane",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([jane,favoriteContact]);
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===favoriteContact.id?favoriteContact:jane);
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()} initialContactId={favoriteContact.id}/>);
    await screen.findByDisplayValue("Favorite Person");
    const target=screen.getByRole("button",{name:/Favorite Person/});
    expect(target).toHaveAttribute("aria-pressed","true");
  });

  it("requires explicit use of each AI suggestion before saving it",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    vi.mocked(mailClient.enrichContact).mockResolvedValue({suggestions:[{field:"company",value:"Acme",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I work at Acme."}],messagesReviewed:3,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    const evidence=(await screen.findByText("I work at Acme.")).closest("article") as HTMLElement;
    expect(within(evidence).getByText("Company")).toBeInTheDocument();
    expect(within(evidence).getByText("From the email")).toBeInTheDocument();
    expect(mailClient.saveContactProfile).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button",{name:"Use suggestion"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({company:"Acme"})));
    expect(screen.getByRole("button",{name:/Jane Doe/})).toHaveAttribute("aria-pressed","true");
  });

  it("omits the quoted evidence line when it just repeats the suggested value",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    vi.mocked(mailClient.enrichContact).mockResolvedValue({suggestions:[{field:"link",value:"https://www.linkedin.com/in/janedoe",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"https://www.linkedin.com/in/janedoe"}],messagesReviewed:3,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("Link");
    expect(screen.getByText("https://www.linkedin.com/in/janedoe")).toBeInTheDocument();
    expect(screen.queryByText("From the email")).not.toBeInTheDocument();
  });

  it("offers more emails only after the first three produced suggestions",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    vi.mocked(mailClient.enrichContact)
      .mockResolvedValueOnce({suggestions:[{field:"company",value:"Acme",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I work at Acme."}],messagesReviewed:3,hasMore:true})
      .mockResolvedValueOnce({suggestions:[{field:"location",value:"Boston",sourceMessageId:"m2",sourceThreadId:"thread-2",excerpt:"I live in Boston."}],messagesReviewed:9,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");

    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("I work at Acme.");
    expect(screen.getByText("3 emails reviewed")).toBeInTheDocument();
    expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o",null,["company","location","bio","link"],false,undefined);

    fireEvent.click(screen.getByRole("button",{name:"Search more emails"}));
    await screen.findByText("I live in Boston.");
    expect(screen.getByText("12 emails reviewed")).toBeInTheDocument();
    expect(screen.getByText("I work at Acme.")).toBeInTheDocument();
    expect(mailClient.enrichContact).toHaveBeenLastCalledWith(jane.id,"openai","gpt-4o",null,["company","location","bio","link"],true,undefined);
    expect(screen.queryByRole("button",{name:"Search more emails"})).not.toBeInTheDocument();
    expect(mailClient.saveContactProfile).not.toHaveBeenCalled();
  });

  it("only asks the AI to enhance the fields that are empty",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    vi.mocked(mailClient.enrichContact).mockResolvedValue({suggestions:[
      {field:"role",value:"CEO",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I am the CEO."},
      {field:"company",value:"Acme",sourceMessageId:"m2",sourceThreadId:"thread-2",excerpt:"I work at Acme."},
    ],messagesReviewed:3,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    const evidence=(await screen.findByText("I work at Acme.")).closest("article") as HTMLElement;
    // Name and Role already hold values, so they are never asked about, and
    // even a stray answer for them never surfaces.
    expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o",null,["company","location","bio","link"],false,undefined);
    expect(within(evidence).getByText("Company")).toBeInTheDocument();
    expect(screen.queryByText("I am the CEO.")).not.toBeInTheDocument();
    expect(screen.queryByText("CEO")).not.toBeInTheDocument();
  });

  it("does not try to enhance a field filled in but not saved",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    vi.mocked(mailClient.enrichContact).mockResolvedValue({suggestions:[{field:"company",value:"Acme",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I work at Acme."}],messagesReviewed:3,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.change(screen.getByLabelText("Company"),{target:{value:"Typed Co"}});
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("3 emails reviewed");
    expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o",null,["location","bio","link"],false,undefined);
    // Even a stray suggestion for the typed field never surfaces.
    expect(screen.queryByText("I work at Acme.")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Company")).toHaveValue("Typed Co");
    expect(mailClient.saveContactProfile).not.toHaveBeenCalled();
  });

  it("withdraws a suggestion when the user types into its field while enhancing",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    vi.mocked(mailClient.enrichContact).mockResolvedValue({suggestions:[{field:"company",value:"Acme",sourceMessageId:"m1",sourceThreadId:"thread-1",excerpt:"I work at Acme."}],messagesReviewed:3,hasMore:false});
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("I work at Acme.");

    fireEvent.change(screen.getByLabelText("Company"),{target:{value:"Typed Co"}});
    expect(screen.queryByText("I work at Acme.")).not.toBeInTheDocument();
    expect(screen.queryByRole("button",{name:"Use suggestion"})).not.toBeInTheDocument();
    expect(mailClient.saveContactProfile).not.toHaveBeenCalled();

    // Clearing the field again makes the suggestion relevant once more.
    fireEvent.change(screen.getByLabelText("Company"),{target:{value:""}});
    await screen.findByText("I work at Acme.");
    fireEvent.click(screen.getByRole("button",{name:"Use suggestion"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({company:"Acme"})));
  });

  it("says so instead of asking the AI when every field already has a value",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    const filled:ContactProfile={...jane,role:"CEO",company:"Acme",location:"Boston",bio:"Builds things",links:["https://example.com"]};
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([filled]);
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(filled);
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    expect(await screen.findByRole("alert")).toHaveTextContent("Every contact field already has a value.");
    expect(mailClient.enrichContact).not.toHaveBeenCalled();
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
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()} initialContactId={derived.id}/>);
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
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.change(screen.getByRole("textbox",{name:"Search contacts"}),{target:{value:"Founder"}});
    await waitFor(()=>expect(mailClient.listContactProfiles).toHaveBeenLastCalledWith("Founder",500,undefined));
    // Only a cleared field can be enhanced; accepting the suggestion then
    // changes the term they were searched by.
    fireEvent.change(screen.getByLabelText("Role"),{target:{value:""}});
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("I am the CEO.");

    fireEvent.click(screen.getByRole("button",{name:"Use suggestion"}));

    await waitFor(()=>expect(screen.getByDisplayValue("CEO")).toBeInTheDocument());
    expect(screen.getByDisplayValue("Jane Doe")).toBeInTheDocument();
    expect(screen.getByRole("textbox",{name:"Search contacts"})).toHaveValue("Founder");
  });

  it("confirms deletion of a saved profile",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:"Delete contact"}));
    fireEvent.click(screen.getByRole("button",{name:"Confirm"}));
    await waitFor(()=>expect(mailClient.deleteContactProfile).toHaveBeenCalledWith(jane.id));
  });

  it("moves the selection through the ordered contact list with arrow keys",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([jane,favoriteContact,newerContact]);
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===favoriteContact.id?favoriteContact:id===newerContact.id?newerContact:jane);
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");

    const pressed=(name:RegExp)=>expect(screen.getByRole("button",{name})).toHaveAttribute("aria-pressed","true");
    const notPressed=(name:RegExp)=>expect(screen.getByRole("button",{name})).toHaveAttribute("aria-pressed","false");
    pressed(/Jane Doe/);

    // The list selection must move synchronously with each keypress, before
    // the selected profile finishes loading into the editor.
    fireEvent.keyDown(window,{key:"ArrowUp"});
    pressed(/Newer Person/);notPressed(/Jane Doe/);
    await screen.findByDisplayValue("Newer Person");

    fireEvent.keyDown(window,{key:"ArrowUp"});
    pressed(/Favorite Person/);notPressed(/Newer Person/);
    await screen.findByDisplayValue("Favorite Person");

    fireEvent.keyDown(window,{key:"ArrowUp"});
    pressed(/Favorite Person/);
    expect(screen.getByDisplayValue("Favorite Person")).toBeInTheDocument();

    fireEvent.keyDown(window,{key:"ArrowDown"});
    pressed(/Newer Person/);notPressed(/Favorite Person/);
    await screen.findByDisplayValue("Newer Person");

    const search=screen.getByRole("textbox",{name:"Search contacts"});
    search.focus();
    fireEvent.keyDown(search,{key:"ArrowDown"});
    pressed(/Newer Person/);notPressed(/Jane Doe/);
    expect(screen.getByDisplayValue("Newer Person")).toBeInTheDocument();
  });
});
