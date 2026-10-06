import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ContactCard, useContactLookup } from "./ContactCard";
import { mailClient } from "./data/client";
import type { ContactProfile } from "./domain";

vi.mock("./data/client",()=>({mailClient:{getContactProfile:vi.fn(),resolveContactIds:vi.fn(),saveContactProfile:vi.fn(),contactActivity:vi.fn()}}));
vi.mock("@tauri-apps/plugin-opener",()=>({openUrl:vi.fn()}));

const bob:ContactProfile={id:"contact:bob@example.com",displayName:"Bob Lee",role:null,company:"Acme",location:null,bio:null,notes:null,links:[],photoData:null,favorite:false,addresses:["bob@example.com"],sentCount:1,receivedCount:1,lastInteractedAt:null,birthday:null,keepInTouch:{intervalDays:null,startedAt:null,snoozedUntil:null,snoozedAt:null,lastTouchAt:null},keepInTouchDueAt:null};

/** Hosts the card the way its callers do: saved profiles and errors flow back in. */
function Host({ initial, onOpenContact = vi.fn(), facts = [] }: { initial: ContactProfile | null; onOpenContact?: (id: string) => void; facts?: string[] }) {
  const [profile, setProfile] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  return <>
    <ContactCard email="bob@example.com" fallbackName="Bob Lee" profile={profile} facts={facts} onOpenContact={onOpenContact}
      onProfileSaved={(saved) => { setError(null); setProfile(saved); }} onError={setError} />
    {error ? <p role="alert">{error}</p> : null}
  </>;
}

describe("ContactCard",()=>{
  afterEach(()=>{cleanup();vi.clearAllMocks();});

  it("toggles favorite from a heart button and shows an error when saving fails",async()=>{
    vi.mocked(mailClient.saveContactProfile).mockRejectedValueOnce(new Error("Address belongs to another contact")).mockResolvedValueOnce({...bob,favorite:true});
    render(<Host initial={bob}/>);
    const heart=screen.getByRole("button",{name:"Add favorite"});
    expect(heart).toHaveAttribute("aria-pressed","false");
    expect(heart).not.toHaveTextContent("Add favorite");
    fireEvent.click(heart);
    expect(await screen.findByRole("alert")).toHaveTextContent("Address belongs to another contact");
    fireEvent.click(screen.getByRole("button",{name:"Add favorite"}));
    expect(await screen.findByRole("button",{name:"Remove favorite"})).toHaveAttribute("aria-pressed","true");
    expect(mailClient.saveContactProfile).toHaveBeenLastCalledWith({...bob,favorite:true});
  });

  it("copies the person's email address",async()=>{
    const writeText=vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator,"clipboard",{value:{writeText},configurable:true});
    render(<Host initial={bob}/>);
    fireEvent.click(screen.getByRole("button",{name:"Copy email address"}));
    expect(writeText).toHaveBeenCalledWith("bob@example.com");
    expect(await screen.findByRole("button",{name:"Copied email address"})).toBeInTheDocument();
  });

  it("opens the saved profile in the address book from the name",()=>{
    const onOpenContact=vi.fn();
    render(<Host initial={bob} onOpenContact={onOpenContact}/>);
    const heading=screen.getByRole("heading",{name:"Bob Lee"});
    const name=within(heading).getByRole("button",{name:"Bob Lee"});
    expect(name).toHaveAccessibleDescription("Opens in Contacts");
    fireEvent.click(name);
    expect(onOpenContact).toHaveBeenCalledWith(bob.id);
  });

  it("saves an unknown person with a compact button instead of a name link",async()=>{
    vi.mocked(mailClient.saveContactProfile).mockResolvedValue(bob);
    render(<Host initial={null}/>);
    const heading=screen.getByRole("heading",{name:"Bob Lee"});
    expect(within(heading).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByRole("button",{name:"Add favorite"})).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:"Save to contacts"}));
    await waitFor(()=>expect(mailClient.saveContactProfile).toHaveBeenCalledWith(expect.objectContaining({id:null,displayName:"Bob Lee",addresses:["bob@example.com"]})));
    expect(await screen.findByRole("button",{name:"Add favorite"})).toBeInTheDocument();
  });

  it("holds back save and favorite until the lookup finishes",()=>{
    render(<ContactCard email="bob@example.com" fallbackName="Bob Lee" profile={null} loaded={false} facts={[]} onOpenContact={vi.fn()} onProfileSaved={vi.fn()} onError={vi.fn()}/>);
    expect(screen.queryByRole("button",{name:"Save to contacts"})).not.toBeInTheDocument();
    expect(screen.queryByRole("button",{name:"Add favorite"})).not.toBeInTheDocument();
  });

  it("shows the contact URL as a text hyperlink after the address and leaves out the about text",()=>{
    const brian:ContactProfile={...bob,displayName:"Brian Anderson",role:"Vice President of IT",location:"3443 N. Central Ave., Phoenix, AZ 85012",links:["https://upwardprojects.com"],bio:"Runs the quarterly IT steering review."};
    render(<Host initial={brian}/>);
    expect(screen.queryByText("Runs the quarterly IT steering review.")).not.toBeInTheDocument();
    const address=screen.getByText("3443 N. Central Ave., Phoenix, AZ 85012");
    const link=screen.getByRole("link",{name:"upwardprojects.com"});
    expect(link).toHaveAttribute("href","https://upwardprojects.com");
    expect(address.compareDocumentPosition(link)&Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(link);
    expect(openUrl).toHaveBeenCalledWith("https://upwardprojects.com");
  });

  it("says when a keep-in-touch reminder is due and stays quiet otherwise",()=>{
    const kit={...bob.keepInTouch,intervalDays:30};
    const yesterday=new Date();yesterday.setDate(yesterday.getDate()-1);
    const nextWeek=new Date();nextWeek.setDate(nextWeek.getDate()+7);
    const {rerender}=render(<ContactCard email="bob@example.com" fallbackName="Bob Lee" profile={{...bob,keepInTouch:kit,keepInTouchDueAt:yesterday.toISOString()}} facts={[]} onOpenContact={vi.fn()} onProfileSaved={vi.fn()} onError={vi.fn()}/>);
    expect(screen.getByText(/^Keep in touch: overdue since /)).toBeInTheDocument();
    rerender(<ContactCard email="bob@example.com" fallbackName="Bob Lee" profile={{...bob,keepInTouch:kit,keepInTouchDueAt:nextWeek.toISOString()}} facts={[]} onOpenContact={vi.fn()} onProfileSaved={vi.fn()} onError={vi.fn()}/>);
    expect(screen.queryByText(/Keep in touch/)).not.toBeInTheDocument();
  });

  it("puts the job title before the history line",()=>{
    render(<Host initial={{...bob,role:"Managing Director"}} facts={["31 emails since Oct 2024","You last wrote Jun 2025"]}/>);
    const activity=screen.getByText("31 emails since Oct 2024 · You last wrote Jun 2025");
    expect(screen.getByText("Managing Director · Acme").compareDocumentPosition(activity)&Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe("useContactLookup",()=>{
  afterEach(()=>{cleanup();vi.clearAllMocks();});

  it("looks a person up only while enabled and builds history facts from local correspondence",async()=>{
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({"bob@example.com":bob.id});
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(bob);
    vi.mocked(mailClient.contactActivity).mockResolvedValue({sentCount:6,receivedCount:25,threadCount:1,firstAt:"2024-10-04T15:00:00Z",lastSentAt:"2025-06-10T15:00:00Z",recentReceivedAt:[]});
    const {result,rerender}=renderHook(({enabled})=>useContactLookup("bob@example.com",enabled),{initialProps:{enabled:false}});
    expect(mailClient.resolveContactIds).not.toHaveBeenCalled();
    expect(result.current.loaded).toBe(false);

    rerender({enabled:true});
    await waitFor(()=>expect(result.current.loaded).toBe(true));
    expect(result.current.profile).toEqual(bob);
    expect(result.current.facts).toEqual(["31 emails since Oct 2024","You last wrote Jun 2025"]);
    expect(mailClient.contactActivity).toHaveBeenCalledWith(bob.id);
  });

  it("uses the mail-derived contact for someone not yet saved",async()=>{
    vi.mocked(mailClient.resolveContactIds).mockResolvedValue({});
    vi.mocked(mailClient.getContactProfile).mockResolvedValue(null);
    vi.mocked(mailClient.contactActivity).mockResolvedValue({sentCount:0,receivedCount:0,threadCount:0,firstAt:null,lastSentAt:null,recentReceivedAt:[]});
    const {result}=renderHook(()=>useContactLookup("new@example.com",true));
    await waitFor(()=>expect(result.current.loaded).toBe(true));
    expect(mailClient.getContactProfile).toHaveBeenCalledWith("derived:new@example.com");
    expect(mailClient.contactActivity).toHaveBeenCalledWith("derived:new@example.com");
    expect(result.current.facts).toEqual([]);
  });
});
