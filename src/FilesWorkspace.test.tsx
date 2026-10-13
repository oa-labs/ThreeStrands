import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FilesWorkspace, type FilesWorkspaceSnapshot } from "./FilesWorkspace";
import { mailClient } from "./data/client";
import type { WorkspaceFile } from "./domain";

const report: WorkspaceFile = { accountId:"you@example.com", messageId:"files-report",threadId:"report-thread",subject:"Next quarter",sentAt:"2026-10-01T12:00:00Z",sender:"Alice <alice@example.com>",direction:"received",attachment:{id:"report",filename:"forecast.pdf",mimeType:"application/pdf",size:2048},starred:false,availability:"download" };
const copy: WorkspaceFile = {...report,messageId:"files-copy",threadId:"copy-thread",attachment:{...report.attachment,id:"copy"}};
describe("Files workspace", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });
  it("automatically lists distinct attachment occurrences and routes open, download and source actions", async () => {
    vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[report,copy],total:2});
    const open=vi.spyOn(mailClient,"openAttachment").mockResolvedValue();
    const download=vi.spyOn(mailClient,"saveAttachment").mockResolvedValue();
    const source=vi.fn();
    render(<FilesWorkspace accountId={null} accountIds={[report.accountId]} onOpenThread={source} />);
    const list=await screen.findByRole("list",{name:"Mail attachments"});
    await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(2));
    const rail=within(screen.getByRole("complementary",{name:"File context"}));
    expect(screen.getByRole("complementary",{name:"File context"})).toHaveClass("context-panel");
    expect(screen.queryByText("Download required")).not.toBeInTheDocument();
    expect(rail.getByText(/^PDF document/)).toBeInTheDocument();
    expect(rail.queryByText(/application\/pdf/)).not.toBeInTheDocument();
    fireEvent.click(rail.getByRole("button",{name:"Open"}));
    fireEvent.click(rail.getByRole("button",{name:"Save as…"}));
    fireEvent.click(rail.getByRole("button",{name:/Next quarter/}));
    expect(open).toHaveBeenCalledWith(report.messageId,"report");
    expect(download).toHaveBeenCalledWith(report.messageId,"report"); expect(source).toHaveBeenCalledWith("report-thread");
    fireEvent.keyDown(within(list).getAllByRole("button",{name:/^forecast\.pdf/})[0],{key:"j"});
    expect(within(list).getAllByRole("button",{name:/^forecast\.pdf/})[1]).toHaveAttribute("aria-current","true");
  });
  it("queries starred, filename, sender, type, direction, local-day dates and selected accounts without claiming offline retention", async () => {
    const query=vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[],total:0});
    render(<FilesWorkspace accountId={null} accountIds={[report.accountId,"other@example.com"]} onOpenThread={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Search files"),{target:{value:"forecast"}});
    fireEvent.click(screen.getByRole("button",{name:"Filters"}));
    expect(screen.getByLabelText("From date")).toHaveValue("");
    expect(screen.getByLabelText("Through date")).toHaveValue("");
    fireEvent.change(screen.getByLabelText("Sender"),{target:{value:"alice"}});
    fireEvent.change(screen.getByLabelText("File Type"),{target:{value:"document"}});
    fireEvent.change(screen.getByLabelText("Direction"),{target:{value:"received"}});
    fireEvent.change(screen.getByLabelText("Account"),{target:{value:report.accountId}});
    fireEvent.change(screen.getByLabelText("From date"),{target:{value:"2026-10-01"}});
    fireEvent.change(screen.getByLabelText("Through date"),{target:{value:"2026-10-02"}});
    fireEvent.click(screen.getByRole("button",{name:"Starred"}));
    await waitFor(() => expect(query).toHaveBeenLastCalledWith(expect.objectContaining({accountIds:[report.accountId],search:"forecast",sender:"alice",type:"document",direction:"received",starred:true,after:new Date(2026,9,1).toISOString(),before:new Date(2026,9,3).toISOString(),offset:0})));
    fireEvent.click(screen.getByTitle("About Files"));
    expect(screen.getByRole("note",{name:"About Files"})).toHaveTextContent("Stars stay on this device");
  });
  it("re-reads every loaded page when a sync or star change refreshes the list", async () => {
    const make=(index:number):WorkspaceFile=>({...report,messageId:`m${index}`,attachment:{...report.attachment,id:`a${index}`,filename:`file-${index}.pdf`}});
    let rows=Array.from({length:60},(_,index)=>make(index));
    const query=vi.spyOn(mailClient,"listFiles").mockImplementation(async ({offset=0,limit=50})=>({files:rows.slice(offset,offset+limit),total:rows.length}));
    const fileStars=vi.spyOn(mailClient,"fileStars").mockResolvedValue([]);
    const props={accountId:report.accountId,accountIds:[report.accountId],onOpenThread:vi.fn()};
    const view=render(<FilesWorkspace {...props} refreshKey="1" />);
    fireEvent.click(await screen.findByRole("button",{name:"Load more files"}));
    const list=within(screen.getByRole("list",{name:"Mail attachments"}));
    await waitFor(() => expect(list.getAllByRole("listitem")).toHaveLength(60));
    rows=[make(100),...rows.slice(1)];
    query.mockClear();
    view.rerender(<FilesWorkspace {...props} refreshKey="2" />);
    await waitFor(() => expect(list.getByRole("button",{name:/^file-100\.pdf/})).toBeInTheDocument());
    expect(list.queryByRole("button",{name:/^file-0\.pdf/})).not.toBeInTheDocument();
    expect(list.getAllByRole("listitem")).toHaveLength(60);
    expect(query.mock.calls.map(([request]) => request.offset)).toEqual([0,50]);
    rows=rows.slice(0,55);
    query.mockClear();
    act(() => { window.dispatchEvent(new Event("threestrands:files-changed")); });
    await waitFor(() => expect(list.getAllByRole("listitem")).toHaveLength(55));
    expect(query.mock.calls.map(([request]) => request.offset)).toEqual([0,50]);
    expect(screen.getByRole("heading",{name:"55 files"})).toBeInTheDocument();
    expect(fileStars).not.toHaveBeenCalled();
  });
  it("sends only a file row's Enter to the context rail and leaves other buttons to activate natively", async () => {
    vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[report,copy],total:2});
    const focus=vi.fn();
    window.addEventListener("threestrands:focus-context",focus);
    try {
      render(<FilesWorkspace accountId={report.accountId} accountIds={[report.accountId]} onOpenThread={vi.fn()} />);
      const list=within(await screen.findByRole("list",{name:"Mail attachments"}));
      await waitFor(() => expect(list.getAllByRole("listitem")).toHaveLength(2));
      for (const control of [list.getAllByRole("button",{name:"Star forecast.pdf"})[0],screen.getByRole("button",{name:"Starred"})]) {
        expect(fireEvent.keyDown(control,{key:"Enter"})).toBe(true);
      }
      expect(focus).not.toHaveBeenCalled();
      const second=list.getAllByRole("button",{name:/^forecast\.pdf/})[1];
      expect(fireEvent.keyDown(second,{key:"Enter"})).toBe(false);
      expect(second).toHaveAttribute("aria-current","true");
      expect(focus).toHaveBeenCalledTimes(1);
    } finally { window.removeEventListener("threestrands:focus-context",focus); }
  });
  it("rolls back a failed star and updates both visible representations after a successful retry", async () => {
    const query=vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[report],total:1});
    const star=vi.spyOn(mailClient,"setFileStar").mockRejectedValueOnce(new Error("Disk full")).mockResolvedValue();
    vi.spyOn(mailClient,"fileStars").mockResolvedValue([[report.messageId,report.attachment.id]]);
    render(<FilesWorkspace accountId={report.accountId} accountIds={[report.accountId]} onOpenThread={vi.fn()} />);
    const list=within(screen.getByRole("list",{name:"Mail attachments"}));
    fireEvent.click(await list.findByRole("button",{name:"Star forecast.pdf"}));
    await screen.findByRole("alert");
    expect(list.getByRole("button",{name:"Star forecast.pdf"})).toHaveAttribute("aria-pressed","false");
    query.mockResolvedValue({files:[{...report,starred:true}],total:1});
    fireEvent.click(list.getByRole("button",{name:"Star forecast.pdf"}));
    await waitFor(() => expect(screen.getAllByRole("button",{name:"Unstar forecast.pdf"})).toHaveLength(2));
    expect(star).toHaveBeenLastCalledWith(report.accountId,report.messageId,"report",true);
  });
  it("keeps source markup as text and restores file selection and filters after a source-email jump", async () => {
    const hostile={...copy,attachment:{...copy.attachment,filename:'<img src=x onerror=alert(1)>.pdf'}};
    vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[report,hostile],total:2});
    const sessions={current:new Map<string,FilesWorkspaceSnapshot>()};
    const props={accountId:report.accountId,accountIds:[report.accountId],onOpenThread:vi.fn(),sessions};
    const view=render(<FilesWorkspace {...props} />);
    await screen.findByRole("button",{name:/^<img src=x/});
    fireEvent.click(screen.getByRole("button",{name:/^<img src=x/}));
    expect(view.container.querySelector("img")).toBeNull();
    view.unmount();
    render(<FilesWorkspace {...props} />);
    await waitFor(() => expect(within(screen.getByRole("complementary",{name:"File context"})).getByRole("heading",{name:hostile.attachment.filename})).toBeInTheDocument());
  });
  it("keeps secondary filters collapsed and lets users remove active filters without clearing search or stars", async () => {
    const query=vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[],total:0});
    render(<FilesWorkspace accountId={report.accountId} accountIds={[report.accountId]} onOpenThread={vi.fn()} />);
    expect(screen.getByRole("searchbox",{name:"Search files"})).toBeVisible();
    expect(screen.queryByRole("group",{name:"File filters"})).not.toBeInTheDocument();
    expect(screen.getByTitle("About Files").closest("details")).not.toHaveAttribute("open");
    fireEvent.change(screen.getByRole("searchbox",{name:"Search files"}),{target:{value:"forecast"}});
    fireEvent.click(screen.getByRole("button",{name:"Starred"}));
    fireEvent.click(screen.getByRole("button",{name:"Filters"}));
    fireEvent.change(screen.getByLabelText("Sender"),{target:{value:"alice"}});
    fireEvent.change(screen.getByLabelText("File Type"),{target:{value:"document"}});
    fireEvent.keyDown(screen.getByLabelText("Sender"),{key:"Escape"});
    expect(screen.queryByRole("group",{name:"File filters"})).not.toBeInTheDocument();
    expect(screen.getByRole("button",{name:/^Filters/})).toHaveFocus();
    const active=within(screen.getByRole("group",{name:"Active file filters"}));
    fireEvent.click(active.getByRole("button",{name:"Remove Sender: alice filter"}));
    await waitFor(()=>expect(query).toHaveBeenLastCalledWith(expect.objectContaining({search:"forecast",sender:"",type:"document",starred:true})));
    fireEvent.click(active.getByRole("button",{name:"Clear filters"}));
    await waitFor(()=>expect(query).toHaveBeenLastCalledWith(expect.objectContaining({search:"forecast",sender:"",type:"",starred:true})));
    expect(screen.queryByRole("group",{name:"Active file filters"})).not.toBeInTheDocument();
  });
  it("shows clean sender names and keeps addresses in details, without repeating a single account in every row", async () => {
    vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[report],total:1});
    const {container}=render(<FilesWorkspace accountId={report.accountId} accountIds={[report.accountId]} onOpenThread={vi.fn()} />);
    const row=await screen.findByRole("listitem");
    expect(within(row).getByText("Alice")).toHaveAttribute("title",report.sender);
    expect(row).not.toHaveTextContent("alice@example.com");
    expect(row).not.toHaveTextContent(report.accountId);
    expect(within(screen.getByRole("complementary",{name:"File context"})).getByText("alice@example.com")).toBeInTheDocument();
    expect(container.querySelector(".file-list-heading-main")).toHaveTextContent("FilenameSenderDateSize");
    expect(container.querySelector(".context-row-wrap")).toHaveTextContent(report.subject);
  });
  it("labels accounts in the merged view so otherwise identical files remain distinguishable", async () => {
    const other={...copy,accountId:"other@example.com"};
    vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[report,other],total:2});
    render(<FilesWorkspace accountId={null} accountIds={[report.accountId,other.accountId]} onOpenThread={vi.fn()} />);
    await waitFor(()=>expect(screen.getAllByRole("listitem")).toHaveLength(2));
    const rows=screen.getAllByRole("listitem");
    expect(rows[0]).toHaveTextContent(report.accountId); expect(rows[1]).toHaveTextContent(other.accountId);
  });
  it("shows availability only in the selected file's details when it matters", async () => {
    const query=vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[report],total:1});
    const connection=vi.spyOn(navigator,"onLine","get").mockReturnValue(true);
    const props={accountId:report.accountId,accountIds:[report.accountId],onOpenThread:vi.fn()};
    const view=render(<FilesWorkspace {...props} refreshKey="1" />);
    await screen.findByRole("listitem");
    expect(screen.queryByText("Download required")).not.toBeInTheDocument();
    expect(screen.queryByText("Availability not yet known")).not.toBeInTheDocument();
    connection.mockReturnValue(false); act(()=>window.dispatchEvent(new Event("offline")));
    expect(within(screen.getByRole("complementary",{name:"File context"})).getByRole("status")).toHaveTextContent("Connect to the internet to open this file.");
    expect(within(screen.getByRole("listitem")).queryByRole("status")).not.toBeInTheDocument();
    connection.mockReturnValue(true); act(()=>window.dispatchEvent(new Event("online")));
    expect(screen.queryByText("Connect to the internet to open this file.")).not.toBeInTheDocument();
    query.mockResolvedValue({files:[{...report,availability:"offline"}],total:1});
    view.rerender(<FilesWorkspace {...props} refreshKey="2" />);
    expect(await screen.findByText("Available offline")).toBeInTheDocument();
    connection.mockReturnValue(false); act(()=>window.dispatchEvent(new Event("offline")));
    expect(screen.getByRole("button",{name:"Open"})).toBeEnabled();
    expect(screen.queryByText("Connect to the internet to open this file.")).not.toBeInTheDocument();
  });
  it("shows unrestricted dates explicitly and removes date restrictions with the filter chips", async () => {
    const query=vi.spyOn(mailClient,"listFiles").mockResolvedValue({files:[],total:0});
    render(<FilesWorkspace accountId={report.accountId} accountIds={[report.accountId]} onOpenThread={vi.fn()} />);
    fireEvent.click(screen.getByRole("button",{name:"Filters"}));
    for (const label of ["From date","Through date"]) {
      expect(screen.getByLabelText(label)).toHaveValue("");
      expect(screen.getByLabelText(label)).toHaveAttribute("aria-description","Any date");
    }
    expect(screen.getAllByText("Any date")).toHaveLength(2);
    fireEvent.change(screen.getByLabelText("From date"),{target:{value:"2026-10-01"}});
    expect(screen.getByLabelText("From date")).not.toHaveAttribute("aria-description");
    fireEvent.click(screen.getByRole("button",{name:/Remove From:/}));
    expect(screen.getByLabelText("From date")).toHaveValue("");
    await waitFor(()=>expect(query).toHaveBeenCalled());
    expect(query.mock.calls.at(-1)?.[0].after).toBeUndefined();
    expect(query.mock.calls.at(-1)?.[0].before).toBeUndefined();
  });

});
