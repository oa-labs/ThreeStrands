import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ContextPanel } from "./ContextPanel";
import { mailClient } from "./data/client";
import type { Account, ContactProfile, ContactTimelineItem, ThreadDetail } from "./domain";

vi.mock("./data/client",()=>({mailClient:{listContactProfiles:vi.fn(),getContactProfile:vi.fn(),contactTimeline:vi.fn(),saveContactProfile:vi.fn()}}));
vi.mock("@tauri-apps/plugin-opener",()=>({openUrl:vi.fn()}));

const jane:ContactProfile={id:"contact:jane@example.com",displayName:"Jane Doe",role:null,company:"Acme",location:null,bio:null,notes:null,links:[],photoData:null,favorite:false,addresses:["jane@example.com"],sentCount:1,receivedCount:1,lastInteractedAt:null};
const bob:ContactProfile={...jane,id:"contact:bob@example.com",displayName:"Bob Lee",addresses:["bob@example.com"]};
const detail={thread:{id:"thread-1"},messages:[{id:"1",sender:"Jane Doe <jane@example.com>",recipients:["You <you@example.com>","Bob Lee <bob@example.com>"],sentAt:"2026-09-24T00:00:00Z"},{id:"2",sender:"Bob Lee <bob@example.com>",recipients:["You <you@example.com>"],sentAt:"2026-09-25T00:00:00Z"}]} as unknown as ThreadDetail;
const account={email:"you@example.com"} as Account;
const timelineItem=(threadId:string,subject:string):ContactTimelineItem=>({threadId,accountId:"you@example.com",subject,snippet:"",sentAt:"2026-09-20T00:00:00Z",labels:[]});

function renderPanel(overrides:Partial<Parameters<typeof ContextPanel>[0]>={}){
  return render(<ContextPanel detail={detail} accounts={[account]} onOpenThread={vi.fn()} onOpenContact={vi.fn()} {...overrides}/>);
}

describe("ContextPanel",()=>{
  afterEach(()=>{cleanup();vi.clearAllMocks();});
  it("defaults to the latest external sender and lets the reader switch participants",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockImplementation(async query=>query?.includes("jane")?[jane]:[bob]);
    vi.mocked(mailClient.getContactProfile).mockImplementation(async id=>id===jane.id?jane:bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    renderPanel();
    await screen.findByRole("heading",{name:"Bob Lee"});
    const participants=screen.getByRole("group",{name:"Conversation participants"});
    expect(within(participants).getByRole("button",{name:"Bob Lee"})).toHaveAttribute("aria-pressed","true");
    fireEvent.click(within(participants).getByRole("button",{name:"Jane Doe"}));
    await screen.findByRole("heading",{name:"Jane Doe"});
    expect(within(participants).getByRole("button",{name:"Jane Doe"})).toHaveAttribute("aria-pressed","true");
    await waitFor(()=>expect(mailClient.listContactProfiles).toHaveBeenLastCalledWith("jane@example.com",100));
  });

  it("splits email-only participant chips into separately truncated local and domain parts",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([bob]);
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

  it("toggles favorite from a heart button and shows an error when saving fails",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([bob]);
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
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([bob]);
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
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([bob]);
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
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([]);
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
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([brian]);
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
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([bob]);
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([timelineItem("thread-1","This conversation"),timelineItem("thread-2","Budget review")]);
    const onOpenThread=vi.fn();
    renderPanel({onOpenThread});

    const history=await screen.findByRole("region",{name:"Recent emails"});
    expect(within(history).queryByText("This conversation")).not.toBeInTheDocument();
    fireEvent.click(within(history).getByRole("button",{name:/Budget review/}));
    expect(onOpenThread).toHaveBeenCalledWith("thread-2");
  });

  it("places the AI brief and related tasks below the contact card in one panel",async()=>{
    vi.mocked(mailClient.listContactProfiles).mockResolvedValue([bob]);
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
    vi.mocked(mailClient.listContactProfiles).mockImplementation(async query=>query?.includes("bob")?[bobWork]:[]);
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bobWork);
    vi.mocked(mailClient.contactTimeline).mockResolvedValue([]);
    const related=vi.fn(()=>null);
    renderPanel({related});

    expect(related).toHaveBeenCalledWith(null);
    await waitFor(()=>expect(related).toHaveBeenLastCalledWith({contactId:bob.id,email:"bob@example.com",addresses:["bob@example.com","bob@work.example.com"]}));

    fireEvent.click(within(screen.getByRole("group",{name:"Conversation participants"})).getByRole("button",{name:"Jane Doe"}));
    await waitFor(()=>expect(related).toHaveBeenLastCalledWith({contactId:"derived:jane@example.com",email:"jane@example.com",addresses:["jane@example.com"]}));
  });
});
