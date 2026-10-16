import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ContextPanel } from "./ContextPanel";
import { mailClient } from "./data/client";
import { formatHistoryDate } from "./contactContext";
import type { Account, ContactActivity, ContactProfile, ContactTimelineItem, MessageTrust, SenderFamiliarity, SenderHistory, ThreadDetail } from "./domain";
import type { SenderActions } from "./SenderSections";
import { expectContextRows } from "./test/contextRows";

vi.mock("./data/client",()=>({mailClient:{getContactProfile:vi.fn(),resolveContactIds:vi.fn(),contactTimeline:vi.fn(),saveContactProfile:vi.fn(),contactActivity:vi.fn(),contactFiles:vi.fn(),domainContext:vi.fn(),senderHistory:vi.fn(),messageTrust:vi.fn(),senderFamiliarity:vi.fn(),fileStars:vi.fn().mockResolvedValue([]),setFileStar:vi.fn().mockResolvedValue(undefined),openAttachment:vi.fn()}}));
vi.mock("@tauri-apps/plugin-opener",()=>({openUrl:vi.fn()}));

const jane:ContactProfile={id:"contact:jane@example.com",displayName:"Jane Doe",role:null,company:"Acme",location:null,bio:null,notes:null,links:[],photoData:null,favorite:false,addresses:["jane@example.com"],sentCount:1,receivedCount:1,lastInteractedAt:null,birthday:null,keepInTouch:{intervalDays:null,startedAt:null,snoozedUntil:null,snoozedAt:null,lastTouchAt:null},keepInTouchDueAt:null};
const bob:ContactProfile={...jane,id:"contact:bob@example.com",displayName:"Bob Lee",addresses:["bob@example.com"]};
const detail={thread:{id:"thread-1",accountId:"you@example.com",labels:["INBOX"]},messages:[{id:"1",sender:"Jane Doe <jane@example.com>",recipients:["You <you@example.com>","Bob Lee <bob@example.com>"],sentAt:"2026-09-24T00:00:00Z",bodyText:"",bodyHtml:""},{id:"2",sender:"Bob Lee <bob@example.com>",recipients:["You <you@example.com>"],sentAt:"2026-09-25T00:00:00Z",bodyText:"",bodyHtml:""}]} as unknown as ThreadDetail;
const account={email:"you@example.com"} as Account;
const unfamiliar:SenderFamiliarity={sentCount:0,firstSentAt:null,receivedBefore:true,contactName:null,nameMatches:[],knownDomains:[]};
const noActivity:ContactActivity={sentCount:0,receivedCount:0,threadCount:0,firstAt:null,lastSentAt:null,recentReceivedAt:[]};
const timelineItem=(threadId:string,subject:string):ContactTimelineItem=>({threadId,accountId:"you@example.com",contactEmail:"bob@example.com",subject,snippet:"",sentAt:"2026-09-20T00:00:00Z",labels:[]});

/** The panel has settled on a person once their person sections start loading. */
async function personLoaded(contactId:string){
  await waitFor(()=>expect(mailClient.contactFiles).toHaveBeenLastCalledWith(contactId,50));
}

/** Opens the person header's switcher and returns whom it offers, in order. */
function otherPeople(header:HTMLElement){
  fireEvent.click(within(header).getByRole("button",{name:/^\d+ others?$/}));
  return within(within(header).getByRole("list",{name:"Other people"})).getAllByRole("button").map(button=>button.title);
}

function renderPanel(overrides:Partial<Parameters<typeof ContextPanel>[0]>={}){
  return render(<ContextPanel detail={detail} accounts={[account]} onOpenThread={vi.fn()} {...overrides}/>);
}

describe("ContextPanel",()=>{
  beforeEach(()=>{
    localStorage.clear();
    vi.mocked(mailClient.contactActivity).mockResolvedValue(noActivity);
    vi.mocked(mailClient.contactFiles).mockResolvedValue({files:[],total:0});
    vi.mocked(mailClient.domainContext).mockResolvedValue({people:[],threads:[]});
    vi.mocked(mailClient.messageTrust).mockResolvedValue(null);
    vi.mocked(mailClient.senderFamiliarity).mockResolvedValue(unfamiliar);
    vi.mocked(mailClient.resolveContactIds).mockImplementation(async emails=>Object.fromEntries(
      emails.filter(email=>email==="jane@example.com"||email==="bob@example.com")
        .map(email=>[email,`contact:${email}`])));
  });
  afterEach(()=>{cleanup();vi.clearAllMocks();vi.restoreAllMocks();});
  it("defaults to the latest external sender and follows a participant picked in the reader",async()=>{
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const {rerender}=renderPanel();
    await personLoaded(bob.id);
    // The reader picks people and shows their contact card, so the panel has neither.
    expect(screen.queryByRole("region",{name:"Contact"})).not.toBeInTheDocument();
    expect(screen.queryByRole("heading",{name:"Bob Lee"})).not.toBeInTheDocument();
    expect(screen.queryByRole("group",{name:"Conversation participants"})).not.toBeInTheDocument();
    expect(screen.queryByRole("heading",{name:"Participants"})).not.toBeInTheDocument();

    rerender(<ContextPanel detail={detail} accounts={[account]} selectedEmail="Jane@Example.com" onOpenThread={vi.fn()}/>);
    await personLoaded(jane.id);
    expect(mailClient.resolveContactIds).toHaveBeenCalledWith(["jane@example.com"]);

    // Someone who is not on this conversation falls back to the latest sender.
    rerender(<ContextPanel detail={detail} accounts={[account]} selectedEmail="stranger@example.com" onOpenThread={vi.fn()}/>);
    await personLoaded(bob.id);
  });

  describe("following the message being read",()=>{
    const partner:ContactProfile={...jane,id:"contact:ana@partner.org",displayName:"Ana Ruiz",addresses:["ana@partner.org"]};
    const thread={thread:{id:"thread-2",accountId:"you@example.com",labels:["INBOX"]},messages:[
      {id:"a",sender:"Ana Ruiz <ana@partner.org>",recipients:["You <you@example.com>","Pat Kim <pat@example.com>"],sentAt:"2026-09-20T00:00:00Z",bodyText:"",bodyHtml:""},
      {id:"b",sender:"You <you@example.com>",recipients:["Lee Ong <lee@vendor.net>"],sentAt:"2026-09-21T00:00:00Z",bodyText:"",bodyHtml:""},
      {id:"c",sender:"Lee Ong <lee@vendor.net>",recipients:["You <you@example.com>"],sentAt:"2026-09-22T00:00:00Z",bodyText:"",bodyHtml:""},
    ]} as unknown as ThreadDetail;
    const panel=(props:Partial<Parameters<typeof ContextPanel>[0]>)=><ContextPanel detail={thread} accounts={[account]} onOpenThread={vi.fn()} {...props}/>;
    beforeEach(()=>{
      vi.mocked(mailClient.resolveContactIds).mockImplementation(async emails=>Object.fromEntries(
        emails.filter(email=>email==="ana@partner.org").map(email=>[email,partner.id])));
      vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===partner.id?partner:null);
      vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    });

    it("shows the sender of the message being read, or whom the user wrote to",async()=>{
      const {rerender}=render(panel({readingMessageId:"a"}));
      await personLoaded(partner.id);
      expect(screen.getByRole("group",{name:"Showing history with"})).toHaveTextContent("Ana Ruiz");

      rerender(panel({readingMessageId:"b"}));
      await personLoaded("derived:lee@vendor.net");

      // Without a message being read, the latest outside sender stays the default.
      rerender(panel({readingMessageId:null}));
      await personLoaded("derived:lee@vendor.net");
    });

    it("holds a person picked in a message header until the reader moves to another message",async()=>{
      const {rerender}=render(panel({readingMessageId:"c",selectedEmail:"ana@partner.org"}));
      await personLoaded(partner.id);
      rerender(panel({readingMessageId:"c",selectedEmail:"ana@partner.org"}));
      await personLoaded(partner.id);
      // The reader drops a pick once its message is left, so the panel follows the next message.
      rerender(panel({readingMessageId:"b",selectedEmail:null}));
      await personLoaded("derived:lee@vendor.net");
    });

    it("lists the conversation's correspondents before the user's colleagues and switches to the one picked",async()=>{
      const onSelectPerson=vi.fn();
      const {rerender}=render(panel({readingMessageId:"c",onSelectPerson}));
      await personLoaded("derived:lee@vendor.net");
      const header=screen.getByRole("group",{name:"Showing history with"});
      expect(otherPeople(header)).toEqual(["ana@partner.org","pat@example.com"]);
      fireEvent.click(within(header).getByRole("button",{name:/Ana Ruiz/}));
      expect(onSelectPerson).toHaveBeenCalledWith("ana@partner.org");
      await personLoaded(partner.id);
      expect(within(header).queryByRole("list",{name:"Other people"})).not.toBeInTheDocument();

      // Moving to another message lets go of the pick.
      rerender(panel({readingMessageId:"b",onSelectPerson}));
      await personLoaded("derived:lee@vendor.net");
    });

    it("closes the switcher on Escape and returns focus to it",async()=>{
      render(panel({readingMessageId:"c"}));
      await personLoaded("derived:lee@vendor.net");
      const toggle=screen.getByRole("button",{name:"2 others"});
      fireEvent.click(toggle);
      const list=screen.getByRole("list",{name:"Other people"});
      expect(within(list).getAllByRole("button")[0]).toHaveFocus();
      fireEvent.keyDown(list,{key:"ArrowDown"});
      expect(within(list).getAllByRole("button")[1]).toHaveFocus();
      fireEvent.keyDown(list,{key:"Escape"});
      expect(screen.queryByRole("list",{name:"Other people"})).not.toBeInTheDocument();
      expect(toggle).toHaveFocus();
      expect(toggle).toHaveAttribute("aria-expanded","false");
    });
  });

  it("names meeting attendees with a saved contact name even when the message says only a first name",async()=>{
    const andy:ContactProfile={...jane,id:"contact:andy@example.com",displayName:"Andy Example",addresses:["andy@example.com","andy@work.example.com"]};
    const andyDetail={...detail,messages:[
      {...detail.messages[0],sender:"andy <andy@example.com>"},
      {...detail.messages[1],recipients:["You <you@example.com>","A. Example <andy@work.example.com>"]},
    ]} as unknown as ThreadDetail;
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===andy.id?andy:bob);
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({"andy@example.com":andy.id,"andy@work.example.com":andy.id,"bob@example.com":bob.id});
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const related=vi.fn(()=>null);
    renderPanel({detail:andyDetail,related});
    await waitFor(()=>expect(related).toHaveBeenLastCalledWith(expect.anything(),expect.arrayContaining([
      {email:"andy@example.com",name:"Andy Example"},
      {email:"andy@work.example.com",name:"Andy Example"},
    ])));
  });

  it("names meeting attendees with a mail-derived contact name when the participant has no saved contact",async()=>{
    const derived:ContactProfile={...jane,id:"derived:andy@example.com",displayName:"Andy Example",addresses:["andy@example.com"]};
    const andyDetail={...detail,messages:[
      {...detail.messages[0],sender:"andy <andy@example.com>"},
      detail.messages[1],
    ]} as unknown as ThreadDetail;
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===derived.id?derived:id===bob.id?bob:null);
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({"bob@example.com":bob.id});
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const related=vi.fn(()=>null);
    renderPanel({detail:andyDetail,related});
    await waitFor(()=>expect(related).toHaveBeenLastCalledWith(expect.anything(),expect.arrayContaining([{email:"andy@example.com",name:"Andy Example"}])));
    expect(mailClient.getContactProfile).toHaveBeenCalledWith("derived:andy@example.com");
  });

  it("keeps an unquoted comma in a recipient's display name instead of naming them by the suffix",async()=>{
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({});
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(null);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const related=vi.fn(()=>null);
    renderPanel({related,detail:{thread:detail.thread,messages:[
      {id:"1",sender:"You <you@example.com>",recipients:["Daniel O'Connor, CFA® <dan@wealth.example>","Smith, Pat, PhD <pat@lab.example>"],sentAt:"2026-09-24T00:00:00Z",bodyHtml:""},
    ]} as unknown as ThreadDetail});
    await personLoaded("derived:dan@wealth.example");
    expect(related).toHaveBeenLastCalledWith(expect.anything(),[
      {email:"dan@wealth.example",name:"Daniel O'Connor, CFA®"},
      {email:"pat@lab.example",name:"Smith, Pat, PhD"},
    ]);
  });

  it("keeps per-address names when participant contacts cannot be resolved",async()=>{
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    vi.mocked(mailClient.resolveContactIds).mockImplementation(async emails=>{
      if(emails.length>1) throw new Error("offline");
      return {"jane@example.com":jane.id,"jane@work.example.com":jane.id,"bob@example.com":bob.id};
    });
    const warn=vi.spyOn(console,"warn").mockImplementation(()=>undefined);
    const error=vi.spyOn(console,"error").mockImplementation(()=>undefined);
    const twoAddresses={...detail,messages:[...detail.messages,{id:"3",sender:"Jane W. <jane@work.example.com>",recipients:["You <you@example.com>"],sentAt:"2026-09-26T00:00:00Z",bodyHtml:""}]} as unknown as ThreadDetail;
    const related=vi.fn(()=>null);
    renderPanel({detail:twoAddresses,related});
    await personLoaded(jane.id);
    await waitFor(()=>expect(warn).toHaveBeenCalledWith("Participant contact lookup failed:",expect.objectContaining({message:"offline"})));
    expect(related).toHaveBeenLastCalledWith(expect.anything(),expect.arrayContaining([
      {email:"bob@example.com",name:"Bob Lee"},
      // Without the lookup, the second address is not merged into Jane's saved contact.
      {email:"jane@work.example.com",name:"Jane W."},
    ]));
    warn.mockRestore();error.mockRestore();
  });

  it("lists recent emails with the participant, excluding the open conversation",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([timelineItem("thread-1","This conversation"),timelineItem("thread-2","Budget review")]);
    const onOpenThread=vi.fn();
    renderPanel({onOpenThread});

    const history=await screen.findByRole("region",{name:"Recent emails"});
    expectContextRows(history);
    expect(within(history).queryByText("This conversation")).not.toBeInTheDocument();
    const row=within(history).getByRole("button",{name:/Budget review/});
    // Every row is with the selected person, so rows leave out their address.
    expect(row).not.toHaveTextContent("bob@example.com");
    expect(row).toHaveTextContent(formatHistoryDate("2026-09-20T00:00:00Z"));
    // Rows that are emails carry the mail glyph, like files carry theirs.
    expect(row.querySelector(".context-row-icon svg.lucide-mail")).not.toBeNull();
    expect(history).not.toHaveTextContent("you@example.com");
    fireEvent.click(within(history).getByRole("button",{name:/Budget review/}));
    expect(onOpenThread).toHaveBeenCalledWith("thread-2");
  });

  it("groups conversation sections above the person sections",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([timelineItem("thread-2","Budget review")]);
    renderPanel({assist:<section aria-label="Brief">brief</section>,related:()=><section aria-label="Conversation tasks">tasks</section>});
    const panel=screen.getByRole("complementary",{name:"Conversation context"});
    await within(panel).findByRole("region",{name:"Recent emails"});
    const regions=within(panel).getAllByRole("region").map((region)=>region.getAttribute("aria-label")??region.textContent?.split(/\d/)[0].trim());
    expect(regions).toEqual(["Brief","Conversation tasks","Recent emails"]);
  });

  it("uses one heading layout: plain label, then a count badge and actions at the end",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([timelineItem("thread-2","Budget review"),timelineItem("thread-3","Offsite")]);
    renderPanel();
    const history=await screen.findByRole("region",{name:"Recent emails"});
    const header=history.querySelector<HTMLElement>(".context-section-header")!;
    expect(within(header).getByRole("button",{name:"Recent emails"})).toHaveAttribute("aria-expanded","true");
    expect(header.querySelector("h3 .context-count")).toBeNull();
    expect(header.querySelector("h3 svg:not(.lucide-chevron-down):not(.lucide-chevron-right)")).toBeNull();
    expect(header.querySelector(".context-section-header-actions .context-count")).toHaveTextContent("2");
  });

  it("decodes HTML entities in recent email snippets",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([{...timelineItem("thread-2","Fund news"),snippet:"Andreessen Horowitz&#39;s update &amp; more"}]);
    renderPanel();
    const history=await screen.findByRole("region",{name:"Recent emails"});
    expect(history).toHaveTextContent("Andreessen Horowitz's update & more");
    expect(history).not.toHaveTextContent("&#39;");
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
    const {rerender}=renderPanel({related});

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

    rerender(<ContextPanel detail={detail} accounts={[account]} selectedEmail="jane@example.com" related={related} onOpenThread={vi.fn()}/>);
    await waitFor(()=>expect(related).toHaveBeenLastCalledWith(
      {contactId:"derived:jane@example.com",email:"jane@example.com",addresses:["jane@example.com"]},
      expect.arrayContaining([
        {email:"jane@example.com",name:"Jane Doe"},
        {email:"bob@example.com",name:"Bob Lee"},
      ]),
    ));
  });

  it("leaves out recent emails when every email with the person is in this conversation",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([timelineItem("thread-1","This conversation")]);
    vi.mocked(mailClient.contactActivity).mockResolvedValue({...noActivity,receivedCount:31,threadCount:1,firstAt:"2024-10-04T15:00:00Z"});
    renderPanel();
    await personLoaded(bob.id);
    await waitFor(()=>expect(mailClient.contactTimeline).toHaveBeenCalled());
    expect(screen.queryByRole("region",{name:"Recent emails"})).not.toBeInTheDocument();
    expect(screen.queryByText(/is in this conversation/)).not.toBeInTheDocument();
  });

  it("lists files the person sent, opens them, and shows the email they came on",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const file=(index:number)=>({messageId:`m${index}`,threadId:index===1?"thread-1":"thread-9",subject:"Report",sentAt:`2026-0${index}-02T15:00:00Z`,attachment:{id:`a${index}`,filename:`Report ${index}.pdf`,mimeType:"application/pdf",size:2048}});
    vi.mocked(mailClient.contactFiles).mockResolvedValue({files:[file(4),file(3),file(2),file(1)],total:30});
    vi.mocked(mailClient.openAttachment).mockResolvedValue();
    const onShowMessage=vi.fn();
    renderPanel({onShowMessage});

    const files=await screen.findByRole("region",{name:"Files"});
    expectContextRows(files);
    // Show email, then the star at the end, both in the one action cell under the date, as on a message's attachments.
    const reportActions=within(files).getByRole("button",{name:"Star Report 4.pdf"}).closest(".context-row-action")!;
    expect([...reportActions.querySelectorAll("button")].map((button)=>button.getAttribute("aria-label"))).toEqual(["Show the email with Report 4.pdf","Star Report 4.pdf"]);
    // Contact files carry no account; the star names none and the backend uses the message's own.
    vi.mocked(mailClient.fileStars).mockResolvedValueOnce([["m4","a4"]]);
    fireEvent.click(within(files).getByRole("button",{name:"Star Report 4.pdf"}));
    await waitFor(()=>expect(mailClient.setFileStar).toHaveBeenCalledWith(undefined,"m4","a4",true));
    expect(await within(files).findByRole("button",{name:"Unstar Report 4.pdf"})).toBeInTheDocument();
    // A PDF shows the document icon, not a generic file.
    expect(files.querySelector(".context-row-icon [data-attachment-kind]")).toHaveAttribute("data-attachment-kind","document");
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

  it("outlines all replies newest first without a filter or date range",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const longDetail={thread:detail.thread,messages:Array.from({length:6},(_,index)=>({
      id:`m${index}`,
      sender:index%3===2?"You <you@example.com>":"Bob Lee <bob@example.com>",
      recipients:[index%3===2?"Bob Lee <bob@example.com>":"You <you@example.com>"],
      sentAt:`2026-0${index+1}-05T15:00:00Z`,
      bodyText:`Message number ${index}`,
      bodyHtml:"",
    }))} as unknown as ThreadDetail;
    const onShowMessage=vi.fn();
    renderPanel({detail:longDetail,onShowMessage});

    const outline=await screen.findByRole("region",{name:"This thread"});
    expectContextRows(outline);
    expect(outline.querySelector(".context-section-header .context-count")).toHaveTextContent("6");
    expect(outline.querySelector(".context-section-note")).not.toBeInTheDocument();
    expect(within(outline).queryByRole("group",{name:"Thread outline filter"})).not.toBeInTheDocument();
    expect(within(outline).queryByRole("button",{name:"All"})).not.toBeInTheDocument();
    expect(within(outline).queryByRole("button",{name:/Your replies/})).not.toBeInTheDocument();
    for (const row of outline.querySelectorAll(".context-row")) expect(row.querySelector(".context-row-icon svg.lucide-mail")).not.toBeNull();
    const rows=()=>within(outline).getAllByRole("button",{name:/Message number/});
    expect(rows().map((row)=>row.textContent)).toEqual([expect.stringContaining("Message number 5"),expect.stringContaining("Message number 4"),expect.stringContaining("Message number 3")]);
    fireEvent.click(within(outline).getByRole("button",{name:"Show 3 more"}));
    expect(rows().map((row)=>row.textContent)).toEqual([5,4,3,2,1,0].map(index=>expect.stringContaining(`Message number ${index}`)));
    expect(rows()[0]).toHaveTextContent(/^You/);
    expect(rows()[1]).toHaveTextContent(/^Bob Lee/);
    expect(rows()[3]).toHaveTextContent(/^You/);
    expect(rows()[3]).toHaveTextContent(formatHistoryDate(longDetail.messages[2].sentAt));
    fireEvent.click(rows()[3]);
    expect(onShowMessage).toHaveBeenCalledWith("thread-1","m2");
    fireEvent.click(within(outline).getByRole("button",{name:"Show fewer"}));
    expect(rows()).toHaveLength(3);
    cleanup();

    renderPanel();
    await personLoaded(bob.id);
    expect(screen.queryByRole("region",{name:"This thread"})).not.toBeInTheDocument();
  });

  it("shows other people at an organization domain without repeating conversations",async()=>{
    const dana:ContactProfile={...jane,id:"contact:dana@acme.test",displayName:"Dana Ruiz",addresses:["dana@acme.test","dana@acme-mail.test"]};
    const danaDetail={thread:detail.thread,messages:[{id:"1",sender:"Dana Ruiz <dana@acme.test>",recipients:["You <you@example.com>"],sentAt:"2026-09-24T00:00:00Z",bodyHtml:""}]} as unknown as ThreadDetail;
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
    expectContextRows(organization);
    for (const row of organization.querySelectorAll(".context-row")) expect(row.querySelector(".context-row-icon svg.lucide-mail")).not.toBeNull();
    expect(mailClient.domainContext).toHaveBeenCalledWith("acme.test",["dana@acme.test","dana@acme-mail.test"],8);
    expect(organization).toHaveTextContent("Sam Lee, pat@acme.test");
    // The count is the conversations listed (thread-1 and thread-2 are shown elsewhere), not the two people.
    expect(organization.querySelector(".context-section-header .context-count")).toHaveTextContent("1");
    expect(within(organization).getAllByRole("button",{name:/Subject/}).map((button)=>button.textContent)).toEqual([expect.stringContaining("Subject thread-3")]);
    fireEvent.click(within(organization).getByRole("button",{name:/Subject thread-3/}));
    expect(onOpenThread).toHaveBeenCalledWith("thread-3");
  });

  it("does not look up colleagues on the user's own domain or a personal mail provider",async()=>{
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel();
    await personLoaded(bob.id);
    cleanup();

    const friend={...jane,id:"contact:friend@gmail.com",displayName:"Friend",addresses:["friend@gmail.com"]};
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({"friend@gmail.com":friend.id});
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(friend);
    renderPanel({detail:{thread:detail.thread,messages:[{id:"1",sender:"Friend <friend@gmail.com>",recipients:["you@example.com"],sentAt:"2026-09-24T00:00:00Z",bodyHtml:""}]} as unknown as ThreadDetail});
    await personLoaded(friend.id);
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

  describe("while replying",()=>{
    const carol:ContactProfile={...jane,id:"contact:carol@example.com",displayName:"Carol Ng",role:"Buyer",notes:"Prefers mornings",addresses:["carol@example.com"]};
    const toBob={email:"bob@example.com",name:"Bob Lee"};
    const ccCarol={email:"carol@example.com",name:null};
    const reply=(recipients:{email:string;name:string|null}[],extra:Partial<NonNullable<Parameters<typeof ContextPanel>[0]["reply"]>>={})=>({recipients,...extra});
    beforeEach(()=>{
      vi.mocked(mailClient.resolveContactIds).mockImplementation(async emails=>Object.fromEntries(
        emails.filter(email=>["jane@example.com","bob@example.com","carol@example.com"].includes(email)).map(email=>[email,`contact:${email}`])));
      vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>({[jane.id]:jane,[bob.id]:bob,[carol.id]:carol})[id]??null);
      vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
      vi.mocked(mailClient.contactActivity).mockResolvedValue({...noActivity,sentCount:3,receivedCount:4,threadCount:2});
    });

    it("heads the person sections with the first To recipient and switches among the reply's recipients",async()=>{
      renderPanel({selectedEmail:null,reply:reply([toBob,ccCarol])});
      const panel=screen.getByRole("complementary",{name:"Conversation context"});
      expect(await within(panel).findByRole("region",{name:"About Bob Lee"})).toBeInTheDocument();
      await personLoaded(bob.id);
      expect(mailClient.contactActivity).not.toHaveBeenCalled();
      const header=within(panel).getByRole("group",{name:"Showing history with"});
      expect(header).toHaveTextContent("Bob Lee");
      expect(otherPeople(header)).toEqual(["carol@example.com"]);

      fireEvent.click(within(header).getByRole("button",{name:"carol@example.com"}));
      const card=await within(panel).findByRole("region",{name:"About Carol Ng"});
      expect(card).toHaveTextContent("Buyer");
      expect(card).toHaveTextContent("Prefers mornings");
      await personLoaded(carol.id);
    });

    it("follows recipients the reply gains or loses, not the conversation's participants",async()=>{
      const {rerender}=renderPanel({reply:reply([toBob])});
      await personLoaded(bob.id);
      // One recipient leaves nobody to switch to; Jane is on the conversation but not on the reply.
      const header=screen.getByRole("group",{name:"Showing history with"});
      expect(within(header).queryByRole("button",{name:/other/})).not.toBeInTheDocument();

      rerender(<ContextPanel detail={detail} accounts={[account]} reply={reply([toBob,ccCarol])} onOpenThread={vi.fn()}/>);
      expect(otherPeople(header)).toEqual(["carol@example.com"]);
      fireEvent.click(within(header).getByRole("button",{name:"carol@example.com"}));
      await personLoaded(carol.id);

      // Removing the chosen recipient returns the panel to the first one left.
      rerender(<ContextPanel detail={detail} accounts={[account]} reply={reply([toBob])} onOpenThread={vi.fn()}/>);
      await personLoaded(bob.id);

      rerender(<ContextPanel detail={detail} accounts={[account]} reply={reply([])} onOpenThread={vi.fn()}/>);
      expect(await screen.findByText("Add a recipient to see your history with them.")).toBeInTheDocument();
      expect(screen.queryByRole("region",{name:/^About /})).not.toBeInTheDocument();
    });

    it("selects a recipient picked in the reader and ignores a pick who is not on the reply",async()=>{
      const {rerender}=renderPanel({selectedEmail:"jane@example.com",reply:reply([toBob,ccCarol])});
      await personLoaded(bob.id);
      expect(screen.queryByRole("region",{name:"About Jane Doe"})).not.toBeInTheDocument();

      rerender(<ContextPanel detail={detail} accounts={[account]} selectedEmail="Carol@Example.com" reply={reply([toBob,ccCarol])} onOpenThread={vi.fn()}/>);
      expect(await screen.findByRole("region",{name:"About Carol Ng"})).toBeInTheDocument();
      await personLoaded(carol.id);
    });

    it("puts the checks and availability first, and the recipient card with the person sections below the conversation's",async()=>{
      vi.mocked(mailClient.contactTimeline).mockResolvedValue([timelineItem("thread-2","Budget review")]);
      renderPanel({
        reply:reply([toBob],{
          checks:<section aria-label="Before you send">checks</section>,
          availability:<section aria-label="Availability">times</section>,
        }),
        assist:<section aria-label="Brief">brief</section>,
        related:()=><section aria-label="Conversation tasks">tasks</section>,
      });
      const panel=screen.getByRole("complementary",{name:"Conversation context"});
      await within(panel).findByRole("region",{name:"Recent emails"});
      await within(panel).findByRole("region",{name:"About Bob Lee"});
      const regions=within(panel).getAllByRole("region").map((region)=>region.getAttribute("aria-label")??region.textContent?.split(/\d/)[0].trim());
      expect(regions).toEqual(["Before you send","Availability","Brief","Conversation tasks","About Bob Lee","Recent emails"]);
      // The brief stays open below the card rather than collapsing while replying.
      expect(within(panel).getByRole("region",{name:"Brief"})).toBeVisible();
    });

    it("names the reply's recipients as meeting attendees",async()=>{
      const related=vi.fn(()=>null);
      renderPanel({reply:reply([toBob,ccCarol]),related});
      await waitFor(()=>expect(related).toHaveBeenLastCalledWith(expect.anything(),expect.arrayContaining([
        {email:"carol@example.com",name:"carol@example.com"},
        {email:"bob@example.com",name:"Bob Lee"},
      ])));
    });

    it("leaves the recipient sections and the activity lookup out while reading",async()=>{
      renderPanel();
      await personLoaded(bob.id);
      expect(screen.queryByRole("region",{name:/^About /})).not.toBeInTheDocument();
      expect(screen.queryByText("Add a recipient to see your history with them.")).not.toBeInTheDocument();
      expect(mailClient.contactActivity).not.toHaveBeenCalled();
    });
  });
  describe("bulk mail",()=>{
    const news="news@launch.example";
    const newsDetail=(overrides:{labels?:string[];unsubscribe?:boolean}={})=>({
      thread:{id:"news-3",accountId:"you@example.com",labels:overrides.labels??["INBOX"]},
      messages:[{id:"n3",sender:`Launch News <${news}>`,recipients:["You <you@example.com>"],sentAt:"2026-10-07T09:00:00Z",
        bodyText:"",bodyHtml:'<a href="https://launch.example/post">Read</a>',
        unsubscribe:overrides.unsubscribe===false?null:{methods:["oneClick"],listId:null}}],
    }) as unknown as ThreadDetail;
    const history:SenderHistory={receivedCount:4,readCount:1,firstAt:"2026-06-02T09:00:00Z",lastSentAt:null,
      recentReceivedAt:[],inboxThreadIds:["news-3","news-2","news-1"],
      threads:[{threadId:"news-3",subject:"This week",snippet:"",sentAt:"2026-10-07T09:00:00Z"},{threadId:"news-2",subject:"Last week",snippet:"Recap",sentAt:"2026-09-30T09:00:00Z"}]};
    const verified:MessageTrust={fromDomain:"launch.example",authentication:{spf:"pass",dkim:"pass",dmarc:"pass",dkimDomains:["launch.example"]},replyTo:null};
    const actions=(overrides:Partial<SenderActions>={}):SenderActions=>({splits:[],rules:[],onUnsubscribe:vi.fn(),onArchiveThreads:vi.fn(),onNewSplit:vi.fn(),onMute:vi.fn(),onUnmute:vi.fn(),...overrides});
    beforeEach(()=>{
      vi.mocked(mailClient.getContactProfile).mockResolvedValue(null);
      vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
      vi.mocked(mailClient.senderHistory).mockResolvedValue(history);
      vi.mocked(mailClient.messageTrust).mockResolvedValue(verified);
    });

    it("lists the sender's mail in Recent emails with quiet decision links at the bottom",async()=>{
      const sender=actions();
      renderPanel({detail:newsDetail(),sender,assist:<section aria-label="Brief">brief</section>,related:()=><section aria-label="Conversation tasks">tasks</section>});
      const panel=screen.getByRole("complementary",{name:"Conversation context"});
      const from=await within(panel).findByRole("region",{name:"Recent emails"});
      await within(panel).findByRole("region",{name:"Trust"});
      const regions=within(panel).getAllByRole("region").map((region)=>region.getAttribute("aria-label")??region.textContent?.split(/\d/)[0].trim());
      expect(regions).toEqual(["Brief","Conversation tasks","Trust","Recent emails"]);
      expect(mailClient.senderHistory).toHaveBeenCalledWith("you@example.com",news,20);
      // One simple card: no separate sender card or statistics line.
      expect(within(panel).queryByRole("region",{name:/^From /})).not.toBeInTheDocument();
      expect(from.querySelector(".context-section-note")).toBeNull();
      // The open conversation is not listed again under its own sender.
      expectContextRows(from);
      expect([...from.querySelectorAll(".context-row-title")].map((title)=>title.textContent)).toEqual(["Last week"]);
      // The decisions are links after the rows, never full buttons.
      const decisions=from.querySelector(".context-sender-actions");
      expect(decisions).toBe(from.querySelector(":scope > div")?.lastElementChild);
      for(const control of decisions!.querySelectorAll("button")){
        expect(control).toHaveClass("btn-link");
        expect(control).not.toHaveClass("btn");
      }
      fireEvent.click(within(from).getByRole("button",{name:"Unsubscribe"}));
      expect(sender.onUnsubscribe).toHaveBeenCalled();
      fireEvent.click(within(from).getByRole("button",{name:"New Split…"}));
      expect(sender.onNewSplit).toHaveBeenCalledWith({accountId:"you@example.com",email:news,name:"Launch News"});
      fireEvent.click(within(from).getByRole("button",{name:"Archive 3 in Inbox"}));
      expect(sender.onArchiveThreads).toHaveBeenCalledWith(["news-3","news-2","news-1"]);
      expect(within(from).queryByRole("button",{name:/^Archive/})).not.toBeInTheDocument();
    });

    it("treats the Updates or Promotions category as bulk mail without an unsubscribe option",async()=>{
      for(const label of ["CATEGORY_UPDATES","CATEGORY_PROMOTIONS"]){
        renderPanel({detail:newsDetail({labels:["INBOX",label],unsubscribe:false}),sender:actions({onUnsubscribe:undefined})});
        const from=await screen.findByRole("region",{name:"Recent emails"});
        expect(within(from).getByRole("button",{name:"Mute…"})).toBeInTheDocument();
        expect(within(from).queryByRole("button",{name:"Unsubscribe"})).not.toBeInTheDocument();
        cleanup();
      }
    });

    it("names the split that already takes the sender's mail instead of offering a new one",async()=>{
      const split={id:"s",name:"Launch",matchKind:"domain" as const,matchValue:"launch.example",sortOrder:0,createdAt:"",accountId:"you@example.com"};
      renderPanel({detail:newsDetail(),sender:actions({splits:[split]})});
      const from=await screen.findByRole("region",{name:"Recent emails"});
      expect(from).toHaveTextContent("Sorted into Launch");
      expect(within(from).queryByRole("button",{name:"New Split…"})).not.toBeInTheDocument();
    });

    it("offers Mute with the Inbox conversations to archive, and Unmute once muted",async()=>{
      const sender=actions();
      renderPanel({detail:newsDetail(),sender});
      const from=await screen.findByRole("region",{name:"Recent emails"});
      fireEvent.click(within(from).getByRole("button",{name:"Mute…"}));
      expect(sender.onMute).toHaveBeenCalledWith({accountId:"you@example.com",email:news,name:"Launch News"},["news-3","news-2","news-1"]);
      cleanup();

      const rule={id:"r",accountId:"you@example.com",matchKind:"domain" as const,matchValue:"launch.example",action:"mute" as const,createdAt:"2026-10-01T00:00:00Z"};
      const muted=actions({rules:[rule]});
      renderPanel({detail:newsDetail(),sender:muted});
      const mutedSection=await screen.findByRole("region",{name:"Recent emails"});
      expect(within(mutedSection).getByRole("button",{name:"Unmute"})).toHaveAttribute("title","Muted, with everyone at launch.example");
      expect(within(mutedSection).queryByRole("button",{name:"Mute…"})).not.toBeInTheDocument();
      fireEvent.click(within(mutedSection).getByRole("button",{name:"Unmute"}));
      expect(muted.onUnmute).toHaveBeenCalledWith(rule);
    });

    it("merges the person's conversations with the sender's mail, newest first, without repeats",async()=>{
      vi.mocked(mailClient.contactTimeline).mockResolvedValue([
        {threadId:"news-2",accountId:"you@example.com",contactEmail:news,subject:"Last week",snippet:"Recap",sentAt:"2026-09-30T09:00:00Z",labels:[]},
        {threadId:"order-1",accountId:"you@example.com",contactEmail:news,subject:"Your order shipped",snippet:"",sentAt:"2026-10-01T09:00:00Z",labels:[]},
        {threadId:"survey-1",accountId:"you@example.com",contactEmail:news,subject:"Feedback",snippet:"",sentAt:"2026-08-01T09:00:00Z",labels:[]},
      ]);
      renderPanel({detail:newsDetail(),sender:actions()});
      const recent=await screen.findByRole("region",{name:"Recent emails"});
      await waitFor(()=>expect([...recent.querySelectorAll(".context-row-title")].map((title)=>title.textContent)).toEqual(["Your order shipped","Last week","Feedback"]));
      expect(screen.getAllByRole("region",{name:"Recent emails"})).toHaveLength(1);
      expectContextRows(recent);
    });

    it("keeps the decisions when the sender's history cannot be read",async()=>{
      vi.mocked(mailClient.senderHistory).mockRejectedValue(new Error("offline"));
      const sender=actions();
      renderPanel({detail:newsDetail(),sender});
      const recent=await screen.findByRole("region",{name:"Recent emails"});
      expect(within(recent).queryByRole("button",{name:/^Archive/})).not.toBeInTheDocument();
      fireEvent.click(within(recent).getByRole("button",{name:"Mute…"}));
      expect(sender.onMute).toHaveBeenCalledWith({accountId:"you@example.com",email:news,name:"Launch News"},[]);
    });

    it("leaves the sender section out of personal mail and shows Trust there only when something needs a look",async()=>{
      vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
      renderPanel({sender:actions()});
      await personLoaded(bob.id);
      await waitFor(()=>expect(mailClient.messageTrust).toHaveBeenCalledWith("2"));
      expect(screen.queryByRole("region",{name:"Trust"})).not.toBeInTheDocument();
      expect(document.querySelector(".context-sender-actions")).toBeNull();
      expect(mailClient.senderHistory).not.toHaveBeenCalled();
      cleanup();

      vi.mocked(mailClient.messageTrust).mockResolvedValue({...verified,fromDomain:"example.com",replyTo:"desk@elsewhere.example"});
      renderPanel({sender:actions()});
      const trust=await screen.findByRole("region",{name:"Trust"});
      expect(trust).toHaveTextContent("Replies go elsewhere");
      expect(trust).toHaveTextContent("Reply-To is desk@elsewhere.example");
    });

    it("does not warn about links in the quoted history the reader folds away",async()=>{
      vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
      vi.mocked(mailClient.messageTrust).mockResolvedValue({...verified,fromDomain:"example.com"});
      const signature='Bob Lee<br>Partnerships, Example Co<br><a href="https://scan.relay.example/r?u=partner.example">partner.example</a>';
      const reply=`Fixed now.<br><br>${signature}<br><br>On Mon, Jane Doe wrote:<blockquote>Is the feed fixed? <a href="https://scan.relay.example/r?u=feed.example">feed.example</a></blockquote>`;
      const thread=(earlier:string)=>({...detail,messages:[{...detail.messages[0],bodyHtml:earlier},{...detail.messages[1],bodyHtml:reply}]}) as unknown as ThreadDetail;
      // The thread showed Bob's signature before, so the reader folds it with the quote.
      renderPanel({detail:thread(`<p>Can you check the feed?</p><p>${signature}</p>`)});
      await personLoaded(bob.id);
      await waitFor(()=>expect(mailClient.messageTrust).toHaveBeenCalledWith("2"));
      expect(screen.queryByRole("region",{name:"Trust"})).not.toBeInTheDocument();
      cleanup();

      renderPanel({detail:thread("<p>Can you check the feed?</p>")});
      const trust=await screen.findByRole("region",{name:"Trust"});
      expect(trust).toHaveTextContent("Shows partner.example, opens relay.example");
      expect(trust).not.toHaveTextContent("feed.example");
    });

    it("heads a warning with Be careful, shows the link it is about, and folds the sender checks under Details",async()=>{
      vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
      vi.mocked(mailClient.messageTrust).mockResolvedValue({...verified,fromDomain:"example.com"});
      vi.mocked(mailClient.senderFamiliarity).mockResolvedValue({...unfamiliar,knownDomains:["partner.example"]});
      const href="https://partnr.example/pay";
      const onShowLink=vi.fn();
      renderPanel({onShowLink,detail:{...detail,messages:[detail.messages[0],{...detail.messages[1],bodyHtml:`<p>Please <a href="${href}">pay the invoice</a>.</p>`}]} as unknown as ThreadDetail});
      const trust=await screen.findByRole("region",{name:"Trust"});
      expect(mailClient.senderFamiliarity).toHaveBeenCalledWith("2","bob@example.com","Bob Lee");
      expect(within(trust).getByText("Be careful")).toBeInTheDocument();
      expect(trust).toHaveTextContent("Link looks like partner.example");
      fireEvent.click(within(trust).getByRole("button",{name:"Show link"}));
      expect(onShowLink).toHaveBeenCalledWith("thread-1","2",{href,text:"pay the invoice"});

      const toggle=within(trust).getByRole("button",{name:/^Details/});
      expect(toggle).toHaveAttribute("aria-expanded","false");
      expect(toggle).toHaveTextContent("Sender checks · 1 site");
      expect(within(trust).queryByText("Sender verified")).not.toBeVisible();
      fireEvent.click(toggle);
      expect(within(trust).getByText("Sender verified")).toBeVisible();
    });

    it("shows a verified sender the user writes to as one Known sender line in personal mail",async()=>{
      vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
      vi.mocked(mailClient.messageTrust).mockResolvedValue({...verified,fromDomain:"example.com"});
      vi.mocked(mailClient.senderFamiliarity).mockResolvedValue({...unfamiliar,sentCount:3,firstSentAt:"2025-02-10T00:00:00Z",knownDomains:["example.com"]});
      renderPanel();
      const trust=await screen.findByRole("region",{name:"Trust"});
      expect(within(trust).getByText("Known sender")).toBeInTheDocument();
      expect(within(trust).queryByText("Be careful")).not.toBeInTheDocument();
      expect(trust.querySelectorAll(".context-row:not([hidden] .context-row)")).toHaveLength(1);
      expect(trust).toHaveTextContent("You've written to Bob 3 times");
      expect(trust).toHaveTextContent("Since Feb 2025");
    });

    it("leaves Trust out of personal mail from a stranger with nothing to warn about",async()=>{
      vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
      vi.mocked(mailClient.messageTrust).mockResolvedValue({...verified,fromDomain:"example.com"});
      vi.mocked(mailClient.senderFamiliarity).mockResolvedValue({...unfamiliar,receivedBefore:false});
      renderPanel();
      await personLoaded(bob.id);
      await waitFor(()=>expect(mailClient.senderFamiliarity).toHaveBeenCalled());
      expect(screen.queryByRole("region",{name:"Trust"})).not.toBeInTheDocument();
    });

    it("still reads the headers when the sender's history cannot be read",async()=>{
      vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
      vi.mocked(mailClient.messageTrust).mockResolvedValue({...verified,fromDomain:"example.com",replyTo:"desk@elsewhere.example"});
      vi.mocked(mailClient.senderFamiliarity).mockRejectedValue(new Error("locked"));
      renderPanel();
      const trust=await screen.findByRole("region",{name:"Trust"});
      expect(trust).toHaveTextContent("Replies go elsewhere");
    });
  });
});
