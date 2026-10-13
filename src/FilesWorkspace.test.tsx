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
    expect(rail.getByText("Download required")).toBeInTheDocument();
    fireEvent.click(rail.getByRole("button",{name:"Open"}));
    fireEvent.click(rail.getByRole("button",{name:"Download"}));
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
    fireEvent.change(screen.getByLabelText("Sender"),{target:{value:"alice"}});
    fireEvent.change(screen.getByLabelText("File type"),{target:{value:"document"}});
    fireEvent.change(screen.getByLabelText("Direction"),{target:{value:"received"}});
    fireEvent.change(screen.getByLabelText("Account"),{target:{value:report.accountId}});
    fireEvent.change(screen.getByLabelText("From date"),{target:{value:"2026-10-01"}});
    fireEvent.change(screen.getByLabelText("Through date"),{target:{value:"2026-10-02"}});
    fireEvent.click(screen.getByRole("button",{name:"Starred"}));
    await waitFor(() => expect(query).toHaveBeenLastCalledWith(expect.objectContaining({accountIds:[report.accountId],search:"forecast",sender:"alice",type:"document",direction:"received",starred:true,after:new Date(2026,9,1).toISOString(),before:new Date(2026,9,3).toISOString(),offset:0})));
    expect(screen.getByText(/Stars are stored on this device/)).toBeInTheDocument();
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
});
