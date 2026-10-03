import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ContextPanel } from "./ContextPanel";
import { mailClient } from "./data/client";
import type { Account, ContactActivity, ContactProfile, ContactTimelineItem, ThreadDetail } from "./domain";

vi.mock("./data/client",()=>({mailClient:{getContactProfile:vi.fn(),resolveContactIds:vi.fn(),contactTimeline:vi.fn(),saveContactProfile:vi.fn(),contactActivity:vi.fn(),contactFiles:vi.fn(),domainContext:vi.fn(),openAttachment:vi.fn()}}));
vi.mock("@tauri-apps/plugin-opener",()=>({openUrl:vi.fn()}));

const jane:ContactProfile={id:"contact:jane@example.com",displayName:"Jane Doe",role:null,company:"Acme",location:null,bio:null,notes:null,links:[],photoData:null,favorite:false,addresses:["jane@example.com"],sentCount:1,receivedCount:1,lastInteractedAt:null};
const bob:ContactProfile={...jane,id:"contact:bob@example.com",displayName:"Bob Lee",addresses:["bob@example.com"]};
const detail={thread:{id:"thread-1"},messages:[{id:"1",sender:"Jane Doe <jane@example.com>",recipients:["You <you@example.com>","Bob Lee <bob@example.com>"],sentAt:"2026-09-24T00:00:00Z"},{id:"2",sender:"Bob Lee <bob@example.com>",recipients:["You <you@example.com>"],sentAt:"2026-09-25T00:00:00Z"}]} as unknown as ThreadDetail;
const account={email:"you@example.com"} as Account;
const noActivity:ContactActivity={sentCount:0,receivedCount:0,threadCount:0,firstAt:null,lastSentAt:null,recentReceivedAt:[]};
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
    localStorage.clear();
    vi.mocked(mailClient.contactActivity).mockResolvedValue(noActivity);
    vi.mocked(mailClient.contactFiles).mockResolvedValue({files:[],total:0});
    vi.mocked(mailClient.domainContext).mockResolvedValue({people:[],threads:[]});
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

  it("keeps an unquoted comma in a recipient's display name instead of naming them by the suffix",async()=>{
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({});
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(null);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel({detail:{thread:{id:"thread-1"},messages:[
      {id:"1",sender:"You <you@example.com>",recipients:["Daniel O'Connor, CFA® <dan@wealth.example>","Smith, Pat, PhD <pat@lab.example>"],sentAt:"2026-09-24T00:00:00Z"},
    ]} as unknown as ThreadDetail});
    expect(await screen.findByRole("heading",{name:"Daniel O'Connor, CFA®"})).toBeInTheDocument();
    const participants=screen.getByRole("group",{name:"Conversation participants"});
    expect(within(participants).getAllByRole("button").map((button)=>button.textContent)).toEqual(["DDaniel O'Connor, CFA®","SSmith, Pat, PhD"]);
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
    const brian:ContactProfile={...bob,displayName:"Brian Anderson",role:"Vice President of IT",location:"3443 N. Central Ave., Phoenix, AZ 85012",links:["https://upwardprojects.com"],bio:"Runs the quarterly IT steering review."};
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(brian);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel();
    await screen.findByRole("heading",{name:"Brian Anderson"});
    // The about text stays on the Contacts page; the panel card keeps to identity facts.
    expect(screen.queryByText("Runs the quarterly IT steering review.")).not.toBeInTheDocument();

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

  it("adds history facts to the contact card from local correspondence",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue({...bob,role:"Managing Director",company:"Acme"});
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    vi.mocked(mailClient.contactActivity).mockResolvedValue({...noActivity,sentCount:6,receivedCount:25,threadCount:1,firstAt:"2024-10-04T15:00:00Z",lastSentAt:"2025-06-10T15:00:00Z"});
    renderPanel();
    const card=await screen.findByRole("region",{name:"Contact"});
    const activity=await within(card).findByText("31 emails since Oct 2024 · You last wrote Jun 2025");
    // The job title introduces the person, so it comes before the history line.
    expect(within(card).getByText("Managing Director · Acme").compareDocumentPosition(activity)&Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(mailClient.contactActivity).toHaveBeenCalledWith(bob.id);
  });

  it("says every email is in this conversation only when local history confirms it",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([timelineItem("thread-1","This conversation")]);
    vi.mocked(mailClient.contactActivity).mockResolvedValue({...noActivity,receivedCount:31,threadCount:1,firstAt:"2024-10-04T15:00:00Z"});
    renderPanel();
    const history=await screen.findByRole("region",{name:"Recent emails"});
    expect(history).toHaveTextContent("Every email with Bob is in this conversation.");
    cleanup();

    vi.mocked(mailClient.contactActivity).mockResolvedValue(noActivity);
    renderPanel();
    await screen.findByRole("heading",{name:"Bob Lee"});
    await waitFor(()=>expect(mailClient.contactActivity).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("region",{name:"Recent emails"})).not.toBeInTheDocument();
  });

  it("lists files the person sent, opens them, and shows the email they came on",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const file=(index:number)=>({messageId:`m${index}`,threadId:index===1?"thread-1":"thread-9",subject:"Report",sentAt:`2026-0${index}-02T15:00:00Z`,attachment:{id:`a${index}`,filename:`Report ${index}.pdf`,mimeType:"application/pdf",size:2048}});
    vi.mocked(mailClient.contactFiles).mockResolvedValue({files:[file(4),file(3),file(2),file(1)],total:30});
    vi.mocked(mailClient.openAttachment).mockResolvedValue();
    const onShowMessage=vi.fn();
    renderPanel({onShowMessage});

    const files=await screen.findByRole("region",{name:"Files from Bob"});
    expect(mailClient.contactFiles).toHaveBeenCalledWith(bob.id,50);
    expect(files).toHaveTextContent("30");
    expect(files).toHaveTextContent("Newest 4 of 30");
    expect(within(files).queryByRole("button",{name:/^Report 1\.pdf/})).not.toBeInTheDocument();
    fireEvent.click(within(files).getByRole("button",{name:"Show 1 more"}));
    fireEvent.click(within(files).getByRole("button",{name:/^Report 1\.pdf/}));
    expect(mailClient.openAttachment).toHaveBeenCalledWith("m1","a1");
    fireEvent.click(within(files).getByRole("button",{name:"Show the email with Report 4.pdf"}));
    expect(onShowMessage).toHaveBeenCalledWith("thread-9","m4");
  });

  it("outlines long conversations newest first and filters to the user's replies",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const longDetail={thread:{id:"thread-1"},messages:Array.from({length:6},(_,index)=>({
      id:`m${index}`,
      sender:index%3===2?"You <you@example.com>":"Bob Lee <bob@example.com>",
      recipients:[index%3===2?"Bob Lee <bob@example.com>":"You <you@example.com>"],
      sentAt:`2026-0${index+1}-05T15:00:00Z`,
      bodyText:`Message number ${index}`,
    }))} as unknown as ThreadDetail;
    const onShowMessage=vi.fn();
    renderPanel({detail:longDetail,onShowMessage});

    const outline=await screen.findByRole("region",{name:"This thread"});
    const rows=()=>within(outline).getAllByRole("button",{name:/Message number/});
    expect(rows().map((row)=>row.textContent)).toEqual([expect.stringContaining("Message number 5"),expect.stringContaining("Message number 4"),expect.stringContaining("Message number 3")]);
    fireEvent.click(within(outline).getByRole("button",{name:"Your replies · 2"}));
    expect(within(outline).getByRole("button",{name:"Your replies · 2"})).toHaveAttribute("aria-pressed","true");
    expect(rows().map((row)=>row.textContent)).toEqual([expect.stringMatching(/^You.*Message number 5/),expect.stringMatching(/^You.*Message number 2/)]);
    fireEvent.click(rows()[1]);
    expect(onShowMessage).toHaveBeenCalledWith("thread-1","m2");
    cleanup();

    renderPanel();
    await screen.findByRole("heading",{name:"Bob Lee"});
    expect(screen.queryByRole("region",{name:"This thread"})).not.toBeInTheDocument();
  });

  it("shows other people at an organization domain without repeating conversations",async()=>{
    const dana:ContactProfile={...jane,id:"contact:dana@acme.test",displayName:"Dana Ruiz",addresses:["dana@acme.test","dana@acme-mail.test"]};
    const danaDetail={thread:{id:"thread-1"},messages:[{id:"1",sender:"Dana Ruiz <dana@acme.test>",recipients:["You <you@example.com>"],sentAt:"2026-09-24T00:00:00Z"}]} as unknown as ThreadDetail;
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({"dana@acme.test":dana.id});
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(dana);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([{...timelineItem("thread-2","Shared with Dana"),contactEmail:"dana@acme.test"}]);
    vi.mocked(mailClient.domainContext).mockResolvedValue({
      people:[{email:"sam@acme.test",displayName:"Sam Lee",lastAt:"2026-09-20T00:00:00Z"},{email:"pat@acme.test",displayName:null,lastAt:"2026-09-10T00:00:00Z"}],
      threads:["thread-1","thread-2","thread-3"].map((id)=>({...timelineItem(id,`Subject ${id}`),contactEmail:"sam@acme.test"})),
    });
    const onOpenThread=vi.fn();
    renderPanel({detail:danaDetail,onOpenThread});

    const organization=await screen.findByRole("region",{name:"Others at acme.test"});
    expect(mailClient.domainContext).toHaveBeenCalledWith("acme.test",["dana@acme.test","dana@acme-mail.test"],8);
    expect(organization).toHaveTextContent("Sam Lee, pat@acme.test");
    expect(within(organization).getAllByRole("button",{name:/Subject/}).map((button)=>button.textContent)).toEqual([expect.stringContaining("Subject thread-3")]);
    fireEvent.click(within(organization).getByRole("button",{name:/Subject thread-3/}));
    expect(onOpenThread).toHaveBeenCalledWith("thread-3");
  });

  it("does not look up colleagues on the user's own domain or a personal mail provider",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel();
    await screen.findByRole("heading",{name:"Bob Lee"});
    await waitFor(()=>expect(mailClient.contactActivity).toHaveBeenCalled());
    cleanup();

    const friend={...jane,id:"contact:friend@gmail.com",displayName:"Friend",addresses:["friend@gmail.com"]};
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({"friend@gmail.com":friend.id});
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(friend);
    renderPanel({detail:{thread:{id:"thread-1"},messages:[{id:"1",sender:"Friend <friend@gmail.com>",recipients:["you@example.com"],sentAt:"2026-09-24T00:00:00Z"}]} as unknown as ThreadDetail});
    await screen.findByRole("heading",{name:"Friend"});
    await waitFor(()=>expect(mailClient.contactActivity).toHaveBeenCalledWith(friend.id));
    expect(mailClient.domainContext).not.toHaveBeenCalled();
  });

  it("remembers a collapsed section on this device",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([timelineItem("thread-2","Budget review")]);
    renderPanel();
    const history=await screen.findByRole("region",{name:"Recent emails"});
    const toggle=within(history).getByRole("button",{name:"Recent emails"});
    expect(toggle).toHaveAttribute("aria-expanded","true");
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded","false");
    expect(within(history).queryByRole("button",{name:/Budget review/})).not.toBeInTheDocument();
    cleanup();

    renderPanel();
    const remembered=await screen.findByRole("region",{name:"Recent emails"});
    expect(within(remembered).getByRole("button",{name:"Recent emails"})).toHaveAttribute("aria-expanded","false");
    fireEvent.click(within(remembered).getByRole("button",{name:"Recent emails"}));
    expect(within(remembered).getByRole("button",{name:/Budget review/})).toBeVisible();
    expect(localStorage.getItem("threestrands.contextPanel.collapsedSections")).toBe("[]");
  });
});
