import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ContactProfile } from "./domain";
import { ContactsWorkspace } from "./ContactsWorkspace";
import { mailClient } from "./data/client";

vi.mock("./data/client",()=>({mailClient:{listContactProfiles:vi.fn(),getContactProfile:vi.fn(),saveContactProfile:vi.fn(),deleteContactProfile:vi.fn(),contactTimeline:vi.fn(),enrichContact:vi.fn(),listKeepInTouch:vi.fn(),setKeepInTouch:vi.fn(),snoozeKeepInTouch:vi.fn(),markContacted:vi.fn(),contactFiles:vi.fn(),openAttachment:vi.fn()}}));

const jane:ContactProfile={id:"contact:jane@example.com",displayName:"Jane Doe",role:"Founder",company:null,location:null,bio:null,notes:null,links:[],photoData:null,favorite:false,addresses:["jane@example.com"],sentCount:3,receivedCount:2,lastInteractedAt:"2026-09-20T00:00:00Z",birthday:null,keepInTouch:{intervalDays:null,startedAt:null,snoozedUntil:null,snoozedAt:null,lastTouchAt:null},keepInTouchDueAt:null};
const favoriteContact:ContactProfile={...jane,id:"contact:favorite@example.com",displayName:"Favorite Person",favorite:true,addresses:["favorite@example.com"],lastInteractedAt:"2026-09-10T00:00:00Z"};
// Empty optional fields start collapsed behind an "Add" button.
const field=(label:string)=>{const add=screen.queryByRole("button",{name:`Add ${label.toLowerCase()}`});if(add)fireEvent.click(add);return screen.getByLabelText(label);};
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
    vi.mocked(mailClient.listKeepInTouch).mockResolvedValue([]);
    vi.mocked(mailClient.contactFiles).mockResolvedValue({files:[],total:0});
  });
  afterEach(()=>{cleanup();vi.clearAllMocks();localStorage.clear();});

  it("clears search and returns focus to the selected contact on Escape",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    const search=screen.getByRole("textbox",{name:"Search contacts"});
    search.focus();
    fireEvent.change(search,{target:{value:"jane"}});
    await waitFor(()=>expect(mailClient.listContactProfiles).toHaveBeenLastCalledWith("jane",500,undefined));
    const onWindowEscape=vi.fn();
    window.addEventListener("keydown",onWindowEscape);
    try{fireEvent.keyDown(search,{key:"Escape"});}finally{window.removeEventListener("keydown",onWindowEscape);}
    expect(search).toHaveValue("");
    expect(onWindowEscape).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(screen.getByRole("button",{name:/Jane Doe/}));
    await waitFor(()=>expect(mailClient.listContactProfiles).toHaveBeenLastCalledWith("",500,undefined));
  });

  it("leaves the search box on Escape when no contact is selected",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([]);
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByText(/No contacts found/);
    const search=screen.getByRole("textbox",{name:"Search contacts"});
    search.focus();
    fireEvent.change(search,{target:{value:"zzz"}});
    fireEvent.keyDown(search,{key:"Escape"});
    expect(search).toHaveValue("");
    expect(document.activeElement).not.toBe(search);
  });

  it("searches sent-to and saved contacts, then saves edited details",async()=>{
    const onSaved=vi.fn();
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={onSaved}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.keyDown(window,{key:"/"});
    const search=screen.getByRole("textbox",{name:"Search contacts"});
    expect(document.activeElement).toBe(search);
    fireEvent.change(search,{target:{value:"jane"}});
    await waitFor(()=>expect(mailClient.listContactProfiles).toHaveBeenLastCalledWith("jane",500,undefined));
    fireEvent.change(field("Company"),{target:{value:"Acme"}});
    fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({company:"Acme",addresses:["jane@example.com"]})));
    await waitFor(()=>expect(onSaved).toHaveBeenCalledOnce());
  });

  it("resizes the contact list from the divider and persists the width",async()=>{
    const originalInnerWidth=Object.getOwnPropertyDescriptor(window,"innerWidth");
    Object.defineProperty(window,"innerWidth",{value:1600,configurable:true});
    try{
      const {container}=render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
      await screen.findByDisplayValue("Jane Doe");
      const handle=screen.getByRole("separator",{name:"Resize contact list"});
      expect(handle).toHaveAttribute("aria-controls","contact-list-panel");
      expect(document.getElementById("contact-list-panel")).toHaveAccessibleName("Contact list");
      expect(handle).toHaveAttribute("aria-valuemin","240");
      expect(handle).toHaveAttribute("aria-valuemax","640");
      const body=container.querySelector(".contacts-workspace-body") as HTMLElement;
      const appliedWidth=()=>body.style.getPropertyValue("--contact-list-width");
      expect(appliedWidth()).toBe("400px");

      // The list sits to the left of the handle, so ArrowRight widens it; the
      // handle keeps the keys from reaching the list's own arrow navigation.
      fireEvent.keyDown(handle,{key:"ArrowRight"});
      expect(appliedWidth()).toBe("410px");
      fireEvent.keyDown(handle,{key:"ArrowLeft",shiftKey:true});
      expect(appliedWidth()).toBe("370px");
      fireEvent.keyDown(handle,{key:"Home"});
      expect(appliedWidth()).toBe("240px");
      fireEvent.keyDown(handle,{key:"ArrowLeft"});
      expect(appliedWidth()).toBe("240px");

      handle.setPointerCapture=vi.fn();
      handle.releasePointerCapture=vi.fn();
      fireEvent.pointerDown(handle,{button:0,pointerId:1,clientX:300});
      fireEvent.pointerMove(handle,{pointerId:1,clientX:360});
      expect(appliedWidth()).toBe("300px");
      fireEvent.pointerMove(handle,{pointerId:1,clientX:380});
      expect(appliedWidth()).toBe("320px");
      fireEvent.pointerUp(handle,{pointerId:1,clientX:380});
      await waitFor(()=>expect(localStorage.getItem("threestrands.contactListWidth")).toBe("320"));

      cleanup();
      const remount=render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
      await screen.findByDisplayValue("Jane Doe");
      const remountedBody=remount.container.querySelector(".contacts-workspace-body") as HTMLElement;
      expect(remountedBody.style.getPropertyValue("--contact-list-width")).toBe("320px");
      fireEvent.dblClick(screen.getByRole("separator",{name:"Resize contact list"}));
      expect(remountedBody.style.getPropertyValue("--contact-list-width")).toBe("400px");
      await waitFor(()=>expect(localStorage.getItem("threestrands.contactListWidth")).toBe("400"));
    }finally{
      if(originalInnerWidth)Object.defineProperty(window,"innerWidth",originalInnerWidth);
    }
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

  it("edits links as removable badges without splitting URL punctuation",async()=>{
    const existing="https://example.com/path?a=1,2;b=3";
    const linked={...jane,links:[existing]};
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([linked]);
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(linked);
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    const input=screen.getByRole("textbox",{name:"Links"});
    expect(screen.getByRole("button",{name:`Remove ${existing}`})).toBeInTheDocument();

    fireEvent.change(input,{target:{value:"https://second.example/a,b;c"}});
    fireEvent.keyDown(input,{key:"Enter"});
    expect(input).toHaveValue("");
    expect(screen.getByRole("button",{name:"Remove https://second.example/a,b;c"})).toBeInTheDocument();
    fireEvent.paste(input,{clipboardData:{getData:()=>"https://third.example/one\nhttps://fourth.example/two"}});
    fireEvent.change(input,{target:{value:existing}});
    fireEvent.blur(input);
    expect(screen.getAllByRole("button",{name:/^Remove https:/})).toHaveLength(4);
    fireEvent.click(screen.getByRole("button",{name:`Remove ${existing}`}));
    fireEvent.change(input,{target:{value:"https://fifth.example/last"}});
    fireEvent.blur(input);
    fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({links:["https://second.example/a,b;c","https://third.example/one","https://fourth.example/two","https://fifth.example/last"]})));
  });

  it("copies a link from its badge and reports clipboard failures",async()=>{
    const link="https://example.com/profile";
    const linked={...jane,links:[link]};
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([linked]);
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(linked);
    const writeText=vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("denied"));
    Object.defineProperty(navigator,"clipboard",{value:{writeText},configurable:true});
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:`Copy ${link}`}));
    expect(await screen.findByRole("button",{name:`Copied ${link}`})).toBeInTheDocument();
    expect(writeText).toHaveBeenCalledWith(link);
    fireEvent.click(screen.getByRole("button",{name:`Copied ${link}`}));
    expect(await screen.findByRole("status")).toHaveTextContent("Could not copy link");
    expect(screen.getByRole("button",{name:`Copy ${link}`})).toBeInTheDocument();
  });

  it("shows the selected account and scopes contacts, history, and enrichment",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    const onOpenThread=vi.fn();
    vi.mocked(mailClient.getContactProfile).mockResolvedValue({...jane,sentCount:20,receivedCount:20});
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([{threadId:"work-thread",accountId:"work@example.com",contactEmail:"jane@example.com",subject:"Project",snippet:"Kickoff notes attached",sentAt:"2026-09-20T00:00:00Z",labels:[]}]);
    render(<ContactsWorkspace accountId="work@example.com" onOpenThread={onOpenThread} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    expect(screen.getByText("· work@example.com")).toBeInTheDocument();
    expect(mailClient.listContactProfiles).toHaveBeenCalledWith("",500,"work@example.com");
    expect(mailClient.contactTimeline).toHaveBeenCalledWith(jane.id,0,20,"work@example.com");
    // The row shows what was said; every row is with this person, so the address would repeat.
    expect(screen.getByRole("button",{name:/Project/})).toHaveTextContent("Kickoff notes attached");
    expect(screen.getByText(/^3 sent · 2 received/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:/Project/}));
    expect(onOpenThread).toHaveBeenCalledWith("work-thread");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await waitFor(()=>expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o",null,["company","location","bio","link"],false,"work@example.com","default"));
  });

  it("does not confirm a contact save that failed",async()=>{
    const onSaved=vi.fn();
    vi.mocked(mailClient.saveContactProfile).mockRejectedValueOnce(new Error("Could not save contact"));
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={onSaved}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.change(field("Company"),{target:{value:"Acme"}});
    fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save contact");
    expect(screen.getByRole("region",{name:"Save changes"})).toHaveTextContent("Unsaved changes");
    expect(onSaved).not.toHaveBeenCalled();
  });

  it("offers Save only while profile fields differ from the saved profile",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    expect(screen.queryByRole("region",{name:"Save changes"})).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Role"),{target:{value:"CEO"}});
    expect(screen.getByRole("region",{name:"Save changes"})).toHaveTextContent("Unsaved changes");
    fireEvent.change(screen.getByLabelText("Role"),{target:{value:"Founder"}});
    expect(screen.queryByRole("region",{name:"Save changes"})).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Role"),{target:{value:"CEO"}});
    fireEvent.click(screen.getByRole("button",{name:"Discard"}));
    expect(screen.getByLabelText("Role")).toHaveValue("Founder");
    expect(screen.queryByRole("region",{name:"Save changes"})).not.toBeInTheDocument();
    expect(mailClient.saveContactProfile).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Role"),{target:{value:"CEO"}});
    fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({role:"CEO"})));
    await waitFor(()=>expect(screen.queryByRole("region",{name:"Save changes"})).not.toBeInTheDocument());
  });

  it("does not offer Save for a favorite change, which saves on its own",async()=>{
    vi.mocked(mailClient.saveContactProfile).mockImplementation(async request=>({...jane,favorite:request.favorite}));
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:"Favorite contact"}));
    await waitFor(()=>expect(screen.getByRole("button",{name:"Favorite contact"})).toHaveAttribute("aria-pressed","true"));
    expect(screen.queryByRole("region",{name:"Save changes"})).not.toBeInTheDocument();
  });

  it("keeps Save available for a new contact and discards it on request",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/New contact/}));
    expect(screen.getByRole("region",{name:"Save changes"})).toHaveTextContent("New contact");
    expect(screen.getByLabelText("Company")).toBeInTheDocument();
    expect(screen.getByLabelText("Notes")).toBeInTheDocument();
    expect(screen.queryByRole("button",{name:"Add company"})).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:"Discard"}));
    expect(screen.getByText("Select a contact to see their details")).toBeInTheDocument();
  });

  it("collapses empty optional fields behind Add buttons and shows filled ones",async()=>{
    const filled={...jane,company:"Acme",notes:"Met at the conference"};
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([filled]);
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(filled);
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    expect(screen.getByLabelText("Company")).toHaveValue("Acme");
    expect(screen.getByLabelText("Notes")).toHaveValue("Met at the conference");
    for(const label of ["Location","Birthday","About"])expect(screen.queryByLabelText(label)).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox",{name:"Links"})).not.toBeInTheDocument();
    expect(screen.queryByRole("button",{name:"Add company"})).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button",{name:"Add location"}));
    expect(document.activeElement).toBe(screen.getByLabelText("Location"));
    expect(screen.queryByRole("button",{name:"Add location"})).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:"Add links"}));
    expect(document.activeElement).toBe(screen.getByRole("textbox",{name:"Links"}));

    // Clearing a saved value keeps its field open so the edit can be saved.
    fireEvent.change(screen.getByLabelText("Company"),{target:{value:""}});
    expect(screen.getByLabelText("Company")).toHaveValue("");
  });

  it("closes fields revealed for one contact when another is opened",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([newerContact,jane]);
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:newerContact);
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Newer Person");
    fireEvent.click(screen.getByRole("button",{name:"Add about"}));
    expect(screen.getByLabelText("About")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:/Jane Doe/}));
    await screen.findByDisplayValue("Jane Doe");
    expect(screen.queryByLabelText("About")).not.toBeInTheDocument();
  });

  it("puts profile suggestions and recent emails in the context rail",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    const page=(start:number,count:number)=>Array.from({length:count},(_,index)=>({threadId:`thread-${start+index}`,accountId:"me@example.com",contactEmail:"jane@example.com",subject:`Subject ${start+index}`,snippet:"",sentAt:"2026-09-20T00:00:00Z",labels:[]}));
    vi.mocked(mailClient.contactTimeline).mockImplementation(async(_id,offset)=>offset===0?page(0,20):page(20,2));
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    const rail=screen.getByRole("complementary",{name:"Contact context"});
    expect(within(rail).getByRole("region",{name:"Profile Suggestions"})).toBeInTheDocument();
    expect(within(rail).getByRole("button",{name:/Enhance with AI/})).toBeInTheDocument();
    const recent=await within(rail).findByRole("region",{name:"Recent emails"});
    expect(within(recent).getAllByRole("button",{name:/^Subject/})).toHaveLength(8);
    expect(within(recent).queryByRole("button",{name:"Load older emails"})).not.toBeInTheDocument();
    fireEvent.click(within(recent).getByRole("button",{name:"Show 12 more"}));
    fireEvent.click(within(recent).getByRole("button",{name:"Load older emails"}));
    await waitFor(()=>expect(within(recent).getAllByRole("button",{name:/^Subject/})).toHaveLength(22));
    expect(mailClient.contactTimeline).toHaveBeenLastCalledWith(jane.id,20,20,undefined);
    expect(within(recent).queryByRole("button",{name:"Load older emails"})).not.toBeInTheDocument();
  });

  it("lists the person's files in the rail and opens them or their email",async()=>{
    const onOpenThread=vi.fn();
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([{threadId:"report-thread",accountId:"me@example.com",contactEmail:"jane@example.com",subject:"Report",snippet:"",sentAt:"2026-09-20T00:00:00Z",labels:[]}]);
    vi.mocked(mailClient.contactFiles).mockResolvedValue({files:[{threadId:"report-thread",messageId:"report-message",subject:"Report",sentAt:"2026-09-20T00:00:00Z",attachment:{id:"report-pdf",filename:"September.pdf",mimeType:"application/pdf",size:1200,contentId:null,inline:false}}],total:1});
    vi.mocked(mailClient.openAttachment).mockResolvedValue(undefined);
    render(<ContactsWorkspace onOpenThread={onOpenThread} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    const rail=screen.getByRole("complementary",{name:"Contact context"});
    const files=await within(rail).findByRole("region",{name:"Files"});
    expect(mailClient.contactFiles).toHaveBeenCalledWith(jane.id,expect.any(Number));
    // Files sit above Recent emails, in the same order as the email sidebar.
    expect(files.compareDocumentPosition(within(rail).getByRole("region",{name:"Recent emails"}))&Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(within(files).getByRole("button",{name:/^September\.pdf/}));
    expect(mailClient.openAttachment).toHaveBeenCalledWith("report-message","report-pdf");
    fireEvent.click(within(files).getByRole("button",{name:"Show the email with September.pdf"}));
    expect(onOpenThread).toHaveBeenCalledWith("report-thread");
  });

  it("leaves out the rail when there is no history and AI is off",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    expect(screen.queryByRole("complementary",{name:"Contact context"})).not.toBeInTheDocument();
  });

  it("shows email volume and the last contact date in the header",async()=>{
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    const meta=document.querySelector(".contact-identity-meta");
    expect(meta).toHaveTextContent("3 sent · 2 received");
    expect(meta).toHaveTextContent(/Last contact/);
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
    fireEvent.change(field("Company"),{target:{value:"Unsaved Company"}});

    fireEvent.click(screen.getByRole("button",{name:"Favorite contact"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({id:jane.id,favorite:true,company:null})));
    await waitFor(()=>expect(screen.getByRole("button",{name:"Favorite contact"})).toHaveAttribute("aria-pressed","true"));
    expect(field("Company")).toHaveValue("Unsaved Company");
    expect(within(screen.getByRole("region",{name:"Favorites"})).getByRole("button",{name:/Jane Doe/})).toBeInTheDocument();
    expect(onSaved).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button",{name:"Favorite contact"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenLastCalledWith(expect.objectContaining({id:jane.id,favorite:false,company:null})));
    await waitFor(()=>expect(screen.getByRole("button",{name:"Favorite contact"})).toHaveAttribute("aria-pressed","false"));
    expect(field("Company")).toHaveValue("Unsaved Company");
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
    fireEvent.change(field("Company"),{target:{value:"Unsaved Company"}});

    fireEvent.click(screen.getByRole("button",{name:"Favorite contact"}));

    await waitFor(()=>expect(mailClient.getContactProfile).toHaveBeenCalledWith("contact:saved-jane"));
    expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({id:derived.id,favorite:true,company:null}));
    expect(field("Company")).toHaveValue("Unsaved Company");
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

  it("enhances with the fast model, reasoning off, when one is set",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.model","gpt-4o");
    localStorage.setItem("threestrands.settings.ai.fastModel","gpt-4o-mini");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await waitFor(()=>expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o-mini",null,["company","location","bio","link"],false,undefined,"off"));
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
    expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o",null,["company","location","bio","link"],false,undefined,"default");

    fireEvent.click(screen.getByRole("button",{name:"Search more emails"}));
    await screen.findByText("I live in Boston.");
    expect(screen.getByText("12 emails reviewed")).toBeInTheDocument();
    expect(screen.getByText("I work at Acme.")).toBeInTheDocument();
    expect(mailClient.enrichContact).toHaveBeenLastCalledWith(jane.id,"openai","gpt-4o",null,["company","location","bio","link"],true,undefined,"default");
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
    expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o",null,["company","location","bio","link"],false,undefined,"default");
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
    fireEvent.change(field("Company"),{target:{value:"Typed Co"}});
    fireEvent.click(screen.getByRole("button",{name:/Enhance with AI/}));
    await screen.findByText("3 emails reviewed");
    expect(mailClient.enrichContact).toHaveBeenCalledWith(jane.id,"openai","gpt-4o",null,["location","bio","link"],false,undefined,"default");
    // Even a stray suggestion for the typed field never surfaces.
    expect(screen.queryByText("I work at Acme.")).not.toBeInTheDocument();
    expect(field("Company")).toHaveValue("Typed Co");
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

    fireEvent.change(field("Company"),{target:{value:"Typed Co"}});
    expect(screen.queryByText("I work at Acme.")).not.toBeInTheDocument();
    expect(screen.queryByRole("button",{name:"Use suggestion"})).not.toBeInTheDocument();
    expect(mailClient.saveContactProfile).not.toHaveBeenCalled();

    // Clearing the field again makes the suggestion relevant once more.
    fireEvent.change(field("Company"),{target:{value:""}});
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
    const enhance=screen.getByRole("button",{name:/Enhance with AI/});
    fireEvent.click(enhance);
    expect(await within(enhance.closest("section")!).findByRole("alert")).toHaveTextContent("Every contact field already has a value.");
    expect(mailClient.enrichContact).not.toHaveBeenCalled();
  });

  it("reports enhancement failures and empty results beside the Enhance button",async()=>{
    localStorage.setItem("threestrands.settings.ai.provider","openai");
    localStorage.setItem("threestrands.settings.ai.features",JSON.stringify({contactEnrichment:true}));
    vi.mocked(mailClient.enrichContact).mockRejectedValueOnce("The AI provider returned invalid contact suggestions");
    render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
    await screen.findByDisplayValue("Jane Doe");
    const enhance=screen.getByRole("button",{name:/Enhance with AI/});
    const section=within(enhance.closest("section")!);
    fireEvent.click(enhance);
    expect(await section.findByRole("alert")).toHaveTextContent("The AI provider returned invalid contact suggestions");
    expect(screen.getAllByRole("alert")).toHaveLength(1);

    fireEvent.click(enhance);
    expect(await section.findByRole("status")).toHaveTextContent("No supported profile details were found in the available emails.");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
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
    expect(field("Company")).toHaveValue("Acme");
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

  describe("keep in touch",()=>{
    const today=new Date();
    const localDay=(offset:number)=>new Date(today.getFullYear(),today.getMonth(),today.getDate()+offset,12).toISOString();
    const withKit=(profile:ContactProfile,intervalDays:number|null,keepInTouchDueAt:string|null,extra:Partial<ContactProfile["keepInTouch"]>={}):ContactProfile=>({...profile,keepInTouch:{...profile.keepInTouch,intervalDays,startedAt:intervalDays?localDay(-60):null,...extra},keepInTouchDueAt});

    it("applies a frequency at once without saving unsaved profile edits",async()=>{
      vi.mocked(mailClient.setKeepInTouch).mockResolvedValue([withKit(jane,30,localDay(10))]);
      const onKeepInTouchChanged=vi.fn();
      render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()} onKeepInTouchChanged={onKeepInTouchChanged}/>);
      await screen.findByDisplayValue("Jane Doe");
      fireEvent.change(field("Company"),{target:{value:"Unsaved Co"}});
      const section=screen.getByRole("region",{name:"Keep in Touch"});
      fireEvent.change(within(section).getByRole("combobox",{name:"Frequency"}),{target:{value:"30"}});
      await waitFor(()=>expect(mailClient.setKeepInTouch).toHaveBeenCalledWith([jane.id],30));
      expect(await within(section).findByText(/Due .* · Last contact/)).toBeInTheDocument();
      expect(onKeepInTouchChanged).toHaveBeenCalled();
      expect(mailClient.saveContactProfile).not.toHaveBeenCalled();
      expect(field("Company")).toHaveValue("Unsaved Co");
      expect(within(section).getByText("Saved")).toBeInTheDocument();
      expect(screen.getByRole("region",{name:"Save changes"})).toHaveTextContent("Unsaved changes");
    });

    it("moves to the saved profile when a reminder is set on a mail-derived person",async()=>{
      const derived:ContactProfile={...jane,id:"derived:jane@example.com"};
      const saved=withKit(jane,7,localDay(5));
      vi.mocked(mailClient.listContactProfiles).mockResolvedValue([derived]);
      vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===saved.id?saved:derived);
      vi.mocked(mailClient.setKeepInTouch).mockResolvedValue([saved]);
      render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
      await screen.findByDisplayValue("Jane Doe");
      fireEvent.change(field("Location"),{target:{value:"Typed but unsaved"}});
      fireEvent.change(screen.getByRole("combobox",{name:"Frequency"}),{target:{value:"7"}});
      await waitFor(()=>expect(mailClient.getContactProfile).toHaveBeenCalledWith(saved.id));
      expect(mailClient.setKeepInTouch).toHaveBeenCalledWith([derived.id],7);
      expect(field("Location")).toHaveValue("Typed but unsaved");
      expect(screen.getAllByRole("button",{name:/Jane Doe/})).toHaveLength(1);
    });

    it("rejects a custom interval outside 1 to 730 days before calling the backend",async()=>{
      vi.mocked(mailClient.setKeepInTouch).mockResolvedValue([withKit(jane,730,localDay(700))]);
      render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
      await screen.findByDisplayValue("Jane Doe");
      fireEvent.change(screen.getByRole("combobox",{name:"Frequency"}),{target:{value:"custom"}});
      const days=screen.getByRole("spinbutton",{name:"Every N Days"});
      for(const invalid of ["0","731"]){
        fireEvent.change(days,{target:{value:invalid}});
        fireEvent.click(screen.getByRole("button",{name:"Set Frequency"}));
        expect(await screen.findByRole("alert")).toHaveTextContent("Enter a whole number of days from 1 to 730");
      }
      expect(mailClient.setKeepInTouch).not.toHaveBeenCalled();
      fireEvent.change(days,{target:{value:"730"}});
      fireEvent.keyDown(days,{key:"Enter"});
      await waitFor(()=>expect(mailClient.setKeepInTouch).toHaveBeenCalledWith([jane.id],730));
    });

    it("snoozes, ends a snooze, and logs contact outside email",async()=>{
      const due=withKit(jane,14,localDay(-2));
      const until=new Date(today.getFullYear(),today.getMonth(),today.getDate()+7).toISOString();
      const snoozed=withKit(jane,14,until,{snoozedUntil:until,snoozedAt:localDay(0)});
      vi.mocked(mailClient.getContactProfile).mockResolvedValue(due);
      vi.mocked(mailClient.snoozeKeepInTouch).mockImplementation(async(_id,value)=>value?snoozed:due);
      vi.mocked(mailClient.markContacted).mockResolvedValue(withKit(jane,14,localDay(14),{lastTouchAt:localDay(0)}));
      render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
      const section=await screen.findByRole("region",{name:"Keep in Touch"});
      expect(await within(section).findByText(/Overdue since/)).toBeInTheDocument();

      fireEvent.click(within(section).getByText("Snooze"));
      fireEvent.click(within(section).getByRole("button",{name:"1 Week"}));
      await waitFor(()=>expect(mailClient.snoozeKeepInTouch).toHaveBeenCalledWith(jane.id,until));
      expect(await within(section).findByText(/Snoozed until/)).toBeInTheDocument();

      fireEvent.click(within(section).getByRole("button",{name:"End Snooze"}));
      await waitFor(()=>expect(mailClient.snoozeKeepInTouch).toHaveBeenLastCalledWith(jane.id,null));
      await within(section).findByText(/Overdue since/);

      fireEvent.click(within(section).getByRole("button",{name:"Mark Contacted"}));
      await waitFor(()=>expect(mailClient.markContacted).toHaveBeenCalledWith(jane.id));
      expect(await within(section).findByText(/Due .* · Last contact/)).toBeInTheDocument();
    });

    it("refuses a snooze date that is not after today",async()=>{
      vi.mocked(mailClient.getContactProfile).mockResolvedValue(withKit(jane,14,localDay(3)));
      render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
      const section=await screen.findByRole("region",{name:"Keep in Touch"});
      await within(section).findByText(/Due /);
      fireEvent.click(within(section).getByText("Snooze"));
      const pad=(value:number)=>String(value).padStart(2,"0");
      fireEvent.change(within(section).getByLabelText("Until Date"),{target:{value:`${today.getFullYear()}-${pad(today.getMonth()+1)}-${pad(today.getDate())}`}});
      fireEvent.click(within(section).getByRole("button",{name:"Snooze Until Date"}));
      expect(await within(section).findByRole("alert")).toHaveTextContent("Choose a date after today");
      expect(mailClient.snoozeKeepInTouch).not.toHaveBeenCalled();
    });

    it("lists reminders by when they fall due and upcoming birthdays, with a due count",async()=>{
      const overdue=withKit({...jane,id:"contact:o",displayName:"Olive Overdue",addresses:["o@example.com"]},7,localDay(-3));
      const dueToday=withKit({...jane,id:"contact:t",displayName:"Tara Today",addresses:["t@example.com"]},30,localDay(0));
      const later=withKit({...jane,id:"contact:l",displayName:"Liam Later",addresses:["l@example.com"]},91,localDay(40));
      const birthday:ContactProfile={...jane,id:"contact:b",displayName:"Bea Birthday",addresses:["b@example.com"],birthday:`${String(new Date(today.getFullYear(),today.getMonth(),today.getDate()+3).getMonth()+1).padStart(2,"0")}-${String(new Date(today.getFullYear(),today.getMonth(),today.getDate()+3).getDate()).padStart(2,"0")}`};
      vi.mocked(mailClient.listKeepInTouch).mockResolvedValue([overdue,dueToday,later,birthday]);
      vi.mocked(mailClient.listContactProfiles).mockResolvedValue([jane,overdue]);
      render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
      await screen.findByDisplayValue("Jane Doe");
      const tab=screen.getByRole("tab",{name:/Keep in Touch/});
      expect(within(tab).getByLabelText("2 due")).toBeInTheDocument();
      // The All Contacts list flags only the contact whose reminder is due.
      expect(within(screen.getByRole("button",{name:/Olive Overdue/})).getByLabelText("Due to reconnect")).toBeInTheDocument();
      expect(within(screen.getByRole("button",{name:/Jane Doe/})).queryByLabelText("Due to reconnect")).not.toBeInTheDocument();

      fireEvent.click(tab);
      expect(tab).toHaveAttribute("aria-selected","true");
      expect(within(screen.getByRole("region",{name:"Overdue"})).getByText("Weekly · Overdue since",{exact:false})).toBeInTheDocument();
      expect(within(screen.getByRole("region",{name:"Due Today"})).getByText("Monthly · Due today")).toBeInTheDocument();
      expect(within(screen.getByRole("region",{name:"Later"})).getByText(/Quarterly · Due/)).toBeInTheDocument();
      expect(screen.queryByRole("region",{name:"This Week"})).not.toBeInTheDocument();
      expect(within(screen.getByRole("region",{name:"Upcoming Birthdays"})).getByRole("button",{name:/Bea Birthday/})).toBeInTheDocument();

      fireEvent.change(screen.getByRole("textbox",{name:"Search contacts"}),{target:{value:"tara"}});
      expect(screen.queryByRole("region",{name:"Overdue"})).not.toBeInTheDocument();
      expect(screen.getByRole("region",{name:"Due Today"})).toBeInTheDocument();
    });

    it("lets the owner control the view and reports tab clicks",async()=>{
      const onViewChange=vi.fn();
      const {rerender}=render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()} view="all" onViewChange={onViewChange}/>);
      await screen.findByDisplayValue("Jane Doe");
      fireEvent.click(screen.getByRole("button",{name:"Select"}));
      fireEvent.click(screen.getByRole("tab",{name:/Keep in Touch/}));
      expect(onViewChange).toHaveBeenCalledWith("keepInTouch");
      // Still controlled: nothing changes until the owner passes the new view.
      expect(screen.getByRole("tab",{name:"All Contacts"})).toHaveAttribute("aria-selected","true");
      rerender(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()} view="keepInTouch" onViewChange={onViewChange}/>);
      expect(screen.getByRole("tab",{name:/Keep in Touch/})).toHaveAttribute("aria-selected","true");
      rerender(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()} view="all" onViewChange={onViewChange}/>);
      // Leaving All Contacts ends a bulk selection however the view changed.
      expect(screen.getByRole("button",{name:"Select"})).toHaveAttribute("aria-pressed","false");
    });

    it("opens directly on the keep-in-touch view and explains an empty one",async()=>{
      render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()} initialView="keepInTouch"/>);
      expect(await screen.findByText(/No keep-in-touch reminders yet/)).toBeInTheDocument();
      expect(screen.getByRole("tab",{name:/Keep in Touch/})).toHaveAttribute("aria-selected","true");
    });

    it("sets one frequency for several selected contacts without opening them",async()=>{
      vi.mocked(mailClient.listContactProfiles).mockResolvedValue([jane,favoriteContact,newerContact]);
      vi.mocked(mailClient.setKeepInTouch).mockImplementation(async(ids,days)=>[jane,newerContact].filter(item=>ids.includes(item.id)).map(item=>withKit(item,days,localDay(20))));
      render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
      await screen.findByDisplayValue("Jane Doe");
      fireEvent.click(screen.getByRole("button",{name:"Select"}));
      const bulk=screen.getByRole("combobox",{name:"Keep in Touch Frequency"});
      expect(bulk).toBeDisabled();
      fireEvent.click(screen.getByRole("checkbox",{name:"Select Jane Doe"}));
      fireEvent.click(screen.getByRole("checkbox",{name:"Select Newer Person"}));
      expect(screen.getByText("2 selected")).toBeInTheDocument();
      expect(mailClient.getContactProfile).not.toHaveBeenCalledWith(newerContact.id);
      fireEvent.change(bulk,{target:{value:"30"}});
      await waitFor(()=>expect(mailClient.setKeepInTouch).toHaveBeenCalledWith([jane.id,newerContact.id],30));
      expect(await screen.findByText("Monthly for 2 contacts")).toHaveAttribute("role","status");
      // The open profile was in the batch, so its section shows the new reminder.
      expect(await within(screen.getByRole("region",{name:"Keep in Touch"})).findByRole("combobox",{name:"Frequency"})).toHaveValue("30");
      expect(screen.queryByRole("checkbox",{name:"Select Jane Doe"})).not.toBeInTheDocument();
      expect(mailClient.listKeepInTouch).toHaveBeenCalledTimes(2);
    });

    it("saves the birthday with the profile form",async()=>{
      render(<ContactsWorkspace onOpenThread={vi.fn()} onSaved={vi.fn()}/>);
      await screen.findByDisplayValue("Jane Doe");
      fireEvent.change(field("Birthday"),{target:{value:" 1990-04-02 "}});
      fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
      await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({birthday:"1990-04-02"})));
      fireEvent.change(field("Birthday"),{target:{value:""}});
      fireEvent.click(screen.getByRole("button",{name:/Save contact/}));
      await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenLastCalledWith(expect.objectContaining({birthday:null})));
    });
  });
});
