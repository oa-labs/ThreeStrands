import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ContextPanel } from "./ContextPanel";
import { mailClient } from "./data/client";
import type { Account, ContactProfile, ContactTimelineItem, ThreadDetail } from "./domain";

vi.mock("./data/client",()=>({mailClient:{getContactProfile:vi.fn(),resolveContactIds:vi.fn(),contactTimeline:vi.fn(),saveContactProfile:vi.fn()}}));
vi.mock("@tauri-apps/plugin-opener",()=>({openUrl:vi.fn()}));

const jane:ContactProfile={id:"contact:jane@example.com",displayName:"Jane Doe",role:null,company:"Acme",location:null,bio:null,notes:null,links:[],photoData:null,favorite:false,addresses:["jane@example.com"],sentCount:1,receivedCount:1,lastInteractedAt:null};
const bob:ContactProfile={...jane,id:"contact:bob@example.com",displayName:"Bob Lee",addresses:["bob@example.com"]};
const detail={thread:{id:"thread-1"},messages:[{id:"1",sender:"Jane Doe <jane@example.com>",recipients:["You <you@example.com>","Bob Lee <bob@example.com>"],sentAt:"2026-09-24T00:00:00Z"},{id:"2",sender:"Bob Lee <bob@example.com>",recipients:["You <you@example.com>"],sentAt:"2026-09-25T00:00:00Z"}]} as unknown as ThreadDetail;
const account={email:"you@example.com"} as Account;
const timelineItem=(threadId:string,subject:string):ContactTimelineItem=>({threadId,accountId:"you@example.com",contactEmail:"bob@example.com",subject,snippet:"",sentAt:"2026-09-20T00:00:00Z",labels:[]});

function renderPanel(overrides:Partial<Parameters<typeof ContextPanel>[0]>={}){
  return render(<ContextPanel detail={detail} accounts={[account]} onOpenThread={vi.fn()} onOpenContact={vi.fn()} {...overrides}/>);
}

// JSDOM has no layout. Model wrapped badge rows so scrolling tests exercise
// visibility against a viewport, rather than just checking a method call.
function mockParticipantRows() {
  return vi.spyOn(HTMLElement.prototype,"getBoundingClientRect").mockImplementation(function(this:HTMLElement) {
    let top=0;
    let height=0;
    if(this.classList.contains("context-participants")) {
      top=100;
      height=90;
    } else if(this instanceof HTMLButtonElement && this.parentElement?.classList.contains("context-participants")) {
      const list=this.parentElement;
      const index=Array.from(list.children).indexOf(this);
      top=100+index*32-list.scrollTop;
      height=26;
    }
    return {top,bottom:top+height,left:0,right:200,width:200,height,x:0,y:top,toJSON:()=>({})};
  });
}

const largeDetail={...detail,messages:[
  {...detail.messages[0],recipients:["You <you@example.com>",...Array.from({length:40},(_,index)=>`Person ${index+1} <person${index+1}@example.com>`)]},
  detail.messages[1],
]} as unknown as ThreadDetail;

describe("ContextPanel",()=>{
  beforeEach(()=>{
    vi.mocked(mailClient.resolveContactIds).mockImplementation(async emails=>Object.fromEntries(
      emails.filter(email=>email==="jane@example.com"||email==="bob@example.com")
        .map(email=>[email,`contact:${email}`])));
  });
  afterEach(()=>{cleanup();vi.clearAllMocks();vi.restoreAllMocks();});
  it("defaults to the latest external sender and lets the reader switch participants",async()=>{
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel();
    await screen.findByRole("heading",{name:"Bob Lee"});
    const participants=screen.getByRole("group",{name:"Conversation participants"});
    expect(within(participants).getByRole("button",{name:"Bob Lee"})).toHaveAttribute("aria-pressed","true");
    fireEvent.click(within(participants).getByRole("button",{name:"Jane Doe"}));
    await screen.findByRole("heading",{name:"Jane Doe"});
    expect(within(participants).getByRole("button",{name:"Jane Doe"})).toHaveAttribute("aria-pressed","true");
    await waitFor(()=>expect(mailClient.resolveContactIds).toHaveBeenCalledWith(["jane@example.com"]));
  });

  it("keeps every participant accessible in a long list and reveals the selected sender without scrolling the panel",async()=>{
    mockParticipantRows();
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:id===bob.id?bob:null);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel({detail:largeDetail});
    await screen.findByRole("heading",{name:"Bob Lee"});

    const list=screen.getByRole("group",{name:"Conversation participants"});
    const selected=within(list).getByRole("button",{name:"Bob Lee"});
    expect(list).toHaveAccessibleDescription("Participants · 42");
    expect(within(list).getAllByRole("button")).toHaveLength(42);
    expect(list.scrollTop).toBeGreaterThan(0);
    expect(selected.getBoundingClientRect().bottom).toBeLessThanOrEqual(list.getBoundingClientRect().bottom);
    expect(selected.getBoundingClientRect().top).toBeGreaterThanOrEqual(list.getBoundingClientRect().top);
    expect(screen.getByRole("complementary",{name:"Conversation context"}).scrollTop).toBe(0);
    expect(screen.getByRole("region",{name:"Contact"})).toBeInTheDocument();
  });

  it("reveals keyboard-focused badges in either direction without changing the selected contact",async()=>{
    mockParticipantRows();
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:id===bob.id?bob:null);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel({detail:largeDetail});
    await screen.findByRole("heading",{name:"Bob Lee"});
    const list=screen.getByRole("group",{name:"Conversation participants"});
    const panel=screen.getByRole("complementary",{name:"Conversation context"});
    panel.scrollTop=70;

    const first=within(list).getByRole("button",{name:"Jane Doe"});
    first.focus();
    expect(first).toHaveFocus();
    expect(list.scrollTop).toBe(0);
    expect(first).toHaveAttribute("aria-pressed","false");

    const last=within(list).getByRole("button",{name:"Person 40"});
    last.focus();
    expect(last).toHaveFocus();
    expect(list.scrollTop).toBeGreaterThan(0);
    expect(last.getBoundingClientRect().bottom).toBeLessThanOrEqual(list.getBoundingClientRect().bottom);
    const priorScroll=list.scrollTop;
    last.focus();
    expect(list.scrollTop).toBe(priorScroll);
    expect(within(list).getByRole("button",{name:"Bob Lee"})).toHaveAttribute("aria-pressed","true");
    expect(panel.scrollTop).toBe(70);

    fireEvent.click(first);
    await screen.findByRole("heading",{name:"Jane Doe"});
    expect(list.scrollTop).toBe(0);
    expect(first).toHaveAttribute("aria-pressed","true");
    expect(panel.scrollTop).toBe(70);
  });

  it("reveals the preferred sender when switching conversations",async()=>{
    mockParticipantRows();
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:id===bob.id?bob:null);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const {rerender}=renderPanel({detail:largeDetail});
    await screen.findByRole("heading",{name:"Bob Lee"});
    const list=screen.getByRole("group",{name:"Conversation participants"});
    const previousScroll=list.scrollTop;

    const nextDetail={...largeDetail,thread:{...largeDetail.thread,id:"thread-2"},messages:[
      largeDetail.messages[0],{...largeDetail.messages[1],sender:"Person 20 <person20@example.com>"},
    ]};
    rerender(<ContextPanel detail={nextDetail} accounts={[account]} onOpenThread={vi.fn()} onOpenContact={vi.fn()}/>);
    await screen.findByRole("heading",{name:"Person 20"});
    const selected=within(list).getByRole("button",{name:"Person 20"});
    expect(selected).toHaveAttribute("aria-pressed","true");
    expect(list.scrollTop).toBeLessThan(previousScroll);
    expect(selected.getBoundingClientRect().top).toBeGreaterThanOrEqual(list.getBoundingClientRect().top);
    expect(selected.getBoundingClientRect().bottom).toBeLessThanOrEqual(list.getBoundingClientRect().bottom);
  });

  it("leaves short lists unscrolled and omits the selector for a single participant",async()=>{
    mockParticipantRows();
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const {rerender}=renderPanel();
    await screen.findByRole("heading",{name:"Bob Lee"});
    expect(screen.getByRole("group",{name:"Conversation participants"}).scrollTop).toBe(0);
    expect(screen.getByText("Participants · 2")).toBeInTheDocument();

    const singleDetail={...detail,messages:[{...detail.messages[0],recipients:["You <you@example.com>"]}]};
    rerender(<ContextPanel detail={singleDetail} accounts={[account]} onOpenThread={vi.fn()} onOpenContact={vi.fn()}/>);
    await screen.findByRole("heading",{name:"Jane Doe"});
    expect(screen.queryByRole("group",{name:"Conversation participants"})).not.toBeInTheDocument();
    expect(screen.queryByText(/Participants ·/)).not.toBeInTheDocument();
  });

  it("uses a saved contact name on participant chips even when the message says only a first name",async()=>{
    const andy:ContactProfile={...jane,id:"contact:andy@example.com",displayName:"Andy Example",addresses:["andy@example.com","andy@work.example.com"]};
    const andyDetail={...detail,messages:[
      {...detail.messages[0],sender:"andy <andy@example.com>"},
      {...detail.messages[1],recipients:["You <you@example.com>","A. Example <andy@work.example.com>"]},
    ]} as unknown as ThreadDetail;
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===andy.id?andy:bob);
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({"andy@example.com":andy.id,"andy@work.example.com":andy.id,"bob@example.com":bob.id});
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel({detail:andyDetail});

    const participants=screen.getByRole("group",{name:"Conversation participants"});
    const andyChip=await within(participants).findByRole("button",{name:"Andy Example"});
    expect(within(participants).queryByRole("button",{name:"andy"})).not.toBeInTheDocument();
    expect(andyChip).toHaveAttribute("title","andy@example.com, andy@work.example.com");
    fireEvent.click(andyChip);
    expect(await screen.findByRole("heading",{name:"Andy Example"})).toBeInTheDocument();
  });

  it("uses a mail-derived contact name when the participant has no saved contact",async()=>{
    const derived:ContactProfile={...jane,id:"derived:andy@example.com",displayName:"Andy Example",addresses:["andy@example.com"]};
    const andyDetail={...detail,messages:[
      {...detail.messages[0],sender:"andy <andy@example.com>"},
      detail.messages[1],
    ]} as unknown as ThreadDetail;
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===derived.id?derived:id===bob.id?bob:null);
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({"bob@example.com":bob.id});
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel({detail:andyDetail});

    const participants=screen.getByRole("group",{name:"Conversation participants"});
    expect(await within(participants).findByRole("button",{name:"Andy Example"})).toHaveAttribute("title","andy@example.com");
    expect(mailClient.getContactProfile).toHaveBeenCalledWith("derived:andy@example.com");
  });

  it("splits email-only participant chips into separately truncated local and domain parts",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const bareDetail={...detail,messages:[{...detail.messages[0],sender:"mjacobs@upwardprojects.com"},detail.messages[1]]} as unknown as ThreadDetail;
    renderPanel({detail:bareDetail});
    await screen.findByRole("heading",{name:"Bob Lee"});
    const participants=screen.getByRole("group",{name:"Conversation participants"});
    const chip=within(participants).getByRole("button",{name:"mjacobs@upwardprojects.com"});
    expect(chip).toHaveAttribute("title","mjacobs@upwardprojects.com");
    expect(chip.querySelector(".context-participant-local")).toHaveTextContent("mjacobs");
    expect(chip.querySelector(".context-participant-domain")).toHaveTextContent("upwardprojects.com");
    expect(within(participants).getByRole("button",{name:"Bob Lee"}).querySelector(".context-participant-address")).toBeNull();
  });

  it("shows one chip per saved contact when a person writes from several addresses",async()=>{
    const janeBoth:ContactProfile={...jane,addresses:["jane@example.com","jane@work.example.com"]};
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?janeBoth:bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({"jane@example.com":jane.id,"jane@work.example.com":jane.id,"bob@example.com":bob.id});
    const twoAddresses={...detail,messages:[...detail.messages,{id:"3",sender:"Jane Doe <jane@work.example.com>",recipients:["You <you@example.com>"],sentAt:"2026-09-26T00:00:00Z"}]} as unknown as ThreadDetail;
    renderPanel({detail:twoAddresses});
    await screen.findByRole("heading",{name:"Jane Doe"});
    const participants=screen.getByRole("group",{name:"Conversation participants"});
    await waitFor(()=>expect(within(participants).getAllByRole("button")).toHaveLength(2));
    expect(participants).toHaveAccessibleDescription("Participants · 2");
    const janeChip=within(participants).getByRole("button",{name:"Jane Doe"});
    expect(janeChip).toHaveAttribute("aria-pressed","true");
    expect(janeChip).toHaveAttribute("title","jane@example.com, jane@work.example.com");
    expect(mailClient.resolveContactIds).toHaveBeenCalledWith(["jane@example.com","bob@example.com","jane@work.example.com"]);
  });

  it("keeps one chip per address when participant contacts cannot be resolved",async()=>{
    // Jane writes from two addresses that a successful lookup would merge into
    // one chip, so a failed lookup must leave three chips instead of two.
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    vi.mocked(mailClient.resolveContactIds).mockImplementation(async emails=>{
      if(emails.length>1) throw new Error("offline");
      return {"jane@example.com":jane.id,"jane@work.example.com":jane.id,"bob@example.com":bob.id};
    });
    const warn=vi.spyOn(console,"warn").mockImplementation(()=>undefined);
    const error=vi.spyOn(console,"error").mockImplementation(()=>undefined);
    const twoAddresses={...detail,messages:[...detail.messages,{id:"3",sender:"Jane Doe <jane@work.example.com>",recipients:["You <you@example.com>"],sentAt:"2026-09-26T00:00:00Z"}]} as unknown as ThreadDetail;
    renderPanel({detail:twoAddresses});
    await screen.findByRole("heading",{name:"Jane Doe"});
    const participants=screen.getByRole("group",{name:"Conversation participants"});
    await waitFor(()=>expect(warn).toHaveBeenCalledWith("Participant contact lookup failed:",expect.objectContaining({message:"offline"})));
    const chips=within(participants).getAllByRole("button");
    expect(chips).toHaveLength(3);
    expect(chips.map(chip=>chip.getAttribute("title"))).toEqual(["jane@example.com","bob@example.com","jane@work.example.com"]);
    warn.mockRestore();error.mockRestore();
  });

  it("toggles favorite from a heart button and shows an error when saving fails",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    vi.mocked(mailClient.saveContactProfile).mockRejectedValueOnce(new Error("Address belongs to another contact")).mockResolvedValueOnce({...bob,favorite:true});
    renderPanel();
    await screen.findByRole("heading",{name:"Bob Lee"});
    const heart=screen.getByRole("button",{name:"Add favorite"});
    expect(heart).toHaveAttribute("aria-pressed","false");
    expect(heart).not.toHaveTextContent("Add favorite");
    fireEvent.click(heart);
    expect(await screen.findByRole("alert")).toHaveTextContent("Address belongs to another contact");
    fireEvent.click(screen.getByRole("button",{name:"Add favorite"}));
    expect(await screen.findByRole("button",{name:"Remove favorite"})).toHaveAttribute("aria-pressed","true");
    expect(mailClient.saveContactProfile).toHaveBeenLastCalledWith({...bob,favorite:true});
  });

  it("copies the selected participant email from the contact card",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const writeText=vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator,"clipboard",{value:{writeText},configurable:true});
    renderPanel();
    await screen.findByRole("heading",{name:"Bob Lee"});

    fireEvent.click(screen.getByRole("button",{name:"Copy email address"}));

    expect(writeText).toHaveBeenCalledWith("bob@example.com");
    expect(await screen.findByRole("button",{name:"Copied email address"})).toBeInTheDocument();
  });

  it("opens the saved profile in the address book from the contact name",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const onOpenContact=vi.fn();
    renderPanel({onOpenContact});
    const heading=await screen.findByRole("heading",{name:"Bob Lee"});

    const name=within(heading).getByRole("button",{name:"Bob Lee"});
    expect(name).toHaveAccessibleDescription("Opens in Contacts");
    fireEvent.click(name);

    expect(onOpenContact).toHaveBeenCalledWith(bob.id);
    expect(screen.queryByRole("button",{name:"Open in Contacts"})).not.toBeInTheDocument();
  });

  it("saves an unknown participant with a compact button instead of a name link",async()=>{
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({});
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(null);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    vi.mocked(mailClient.saveContactProfile).mockResolvedValue(bob);
    renderPanel();
    const heading=await screen.findByRole("heading",{name:"Bob Lee"});
    expect(within(heading).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByRole("button",{name:"Add favorite"})).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button",{name:"Save to contacts"}));

    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({id:null,displayName:"Bob Lee",addresses:["bob@example.com"]})));
    expect(await screen.findByRole("button",{name:"Add favorite"})).toBeInTheDocument();
  });

  it("shows the contact URL as a text hyperlink after the address instead of a button",async()=>{
    const brian:ContactProfile={...bob,displayName:"Brian Anderson",role:"Vice President of IT",location:"3443 N. Central Ave., Phoenix, AZ 85012",links:["https://upwardprojects.com"]};
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(brian);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel();
    await screen.findByRole("heading",{name:"Brian Anderson"});

    const address=screen.getByText("3443 N. Central Ave., Phoenix, AZ 85012");
    const link=screen.getByRole("link",{name:"upwardprojects.com"});
    expect(link).toHaveAttribute("href","https://upwardprojects.com");
    expect(screen.queryByRole("button",{name:"upwardprojects.com"})).not.toBeInTheDocument();
    expect(address.compareDocumentPosition(link)&Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    fireEvent.click(link);
    expect(openUrl).toHaveBeenCalledWith("https://upwardprojects.com");
  });

  it("lists recent emails with the participant, excluding the open conversation",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([timelineItem("thread-1","This conversation"),timelineItem("thread-2","Budget review")]);
    const onOpenThread=vi.fn();
    renderPanel({onOpenThread});

    const history=await screen.findByRole("region",{name:"Recent emails"});
    expect(within(history).queryByText("This conversation")).not.toBeInTheDocument();
    expect(within(history).getByRole("button",{name:/Budget review/})).toHaveTextContent("bob@example.com");
    expect(history).not.toHaveTextContent("you@example.com");
    fireEvent.click(within(history).getByRole("button",{name:/Budget review/}));
    expect(onOpenThread).toHaveBeenCalledWith("thread-2");
  });

  it("places the AI brief and related tasks below the contact card in one panel",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel({assist:<section aria-label="Brief">brief</section>,related:()=><section aria-label="Conversation tasks">tasks</section>});
    const panel=screen.getByRole("complementary",{name:"Conversation context"});
    await within(panel).findByRole("heading",{name:"Bob Lee"});
    const regions=within(panel).getAllByRole("region").map((region)=>region.getAttribute("aria-label"));
    expect(regions).toEqual(["Contact","Brief","Conversation tasks"]);
  });

  it("hands related sections the selected person once their contact record is known",async()=>{
    const bobWork={...bob,addresses:["bob@example.com","bob@work.example.com"]};
    vi.mocked(mailClient.resolveContactIds).mockImplementation(async emails=>{
      const owners:Record<string,string>={};
      if(emails.includes("bob@example.com")) owners["bob@example.com"]=bob.id;
      return owners;
    });
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===bob.id?bobWork:null);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const related=vi.fn(()=>null);
    renderPanel({related});

    expect(related).toHaveBeenCalledWith(null, expect.arrayContaining([
      {email:"jane@example.com",name:"Jane Doe"},
      {email:"bob@example.com",name:"Bob Lee"},
    ]));
    await waitFor(()=>expect(related).toHaveBeenLastCalledWith(
      {contactId:bob.id,email:"bob@example.com",addresses:["bob@example.com","bob@work.example.com"]},
      expect.arrayContaining([
        {email:"jane@example.com",name:"Jane Doe"},
        {email:"bob@example.com",name:"Bob Lee"},
        {email:"bob@work.example.com",name:"Bob Lee"},
      ]),
    ));

    fireEvent.click(within(screen.getByRole("group",{name:"Conversation participants"})).getByRole("button",{name:"Jane Doe"}));
    await waitFor(()=>expect(related).toHaveBeenLastCalledWith(
      {contactId:"derived:jane@example.com",email:"jane@example.com",addresses:["jane@example.com"]},
      expect.arrayContaining([
        {email:"jane@example.com",name:"Jane Doe"},
        {email:"bob@example.com",name:"Bob Lee"},
      ]),
    ));
  });
});
