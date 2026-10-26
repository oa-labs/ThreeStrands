import { describe, expect, it } from "vitest";
import { contextQuery, contextTerms, meetingFiles, rankContext, sharedReason } from "./workspaceContextPolicy";
import type { WorkspaceConversation, WorkspaceFile } from "./domain";

const candidate: WorkspaceConversation={threadId:"budget",accountId:"work@example.com",subject:"Atlas budget",sentAt:"2026-10-01T10:00:00Z",participants:["Alice <alice@example.com>"]};
describe("workspace context relevance", () => {
  it("does not invent materials for generic meetings or from attendee overlap alone", () => {
    const query=contextQuery({kind:"meeting",event:{id:"event",accountId:"calendar@example.com",title:"Weekly planning meeting",start:"2026-10-13T10:00:00Z",end:"2026-10-13T11:00:00Z",allDay:false,attendees:["alice@example.com"]}},["work@example.com"]);
    expect(query.accountIds).toEqual(["work@example.com"]);
    expect(query.terms).toEqual([]);
    expect(rankContext([candidate],query)).toEqual([]);
  });
  it("requires topic and participant evidence for a meeting and excludes other accounts", () => {
    const query={accountIds:["work@example.com"],terms:["atlas"],attendees:["alice@example.com"]};
    const result=rankContext([candidate,{...candidate,threadId:"unrelated",subject:"Family photos"},{...candidate,threadId:"wrong-person",participants:["bob@example.com"]},{...candidate,threadId:"private",accountId:"personal@example.com"}],query);
    expect(result).toEqual([expect.objectContaining({threadId:"budget",reason:"Matching topic and attendees"})]);
  });
  it("ranks a direct source above topic matches and uses stable tie breakers", () => {
    const source={...candidate,threadId:"source",subject:"Re: Documents",sentAt:"2020-01-01T00:00:00Z"};
    const query={accountIds:["work@example.com"],threadId:"source",terms:["atlas"],attendees:[]};
    expect(rankContext([candidate,source],query).map((item)=>item.threadId)).toEqual(["source","budget"]);
    expect(contextTerms("Atlas Atlas review <script>alert</script>")).toContain("atlas");
  });
  it("keeps one invitation for a meeting: the one titled like it, or else the newest", () => {
    const file=(id:string,subject:string,sentAt:string,attachment:Partial<WorkspaceFile["attachment"]>={},starred=false):WorkspaceFile=>({accountId:"work@example.com",messageId:id,threadId:id,subject,sentAt,sender:"alice@example.com",direction:"received",attachment:{id:"part:0.1",filename:"invite.ics",mimeType:"text/calendar",size:10,...attachment},starred,availability:"unknown"});
    const week10=file("w10","Invitation: Atlas sync - Week 10 @ Mon Oct 19, 2026","2026-10-12T10:00:00Z");
    const week11=file("w11","Updated invitation: Atlas sync - Week 11","2026-10-05T10:00:00Z");
    // Detected by filename alone, without a calendar MIME type.
    const week12=file("w12","Atlas sync - Week 12","2026-10-20T10:00:00Z",{filename:"meeting.ICS",mimeType:"application/octet-stream"});
    const deck=file("deck","Atlas sync deck","2026-10-01T10:00:00Z",{filename:"atlas.pdf",mimeType:"application/pdf"});
    const starred=file("starred","Invitation: Atlas sync - Week 9","2026-09-28T10:00:00Z",{},true);
    const all=[week10,week11,week12,deck,starred];
    expect(meetingFiles(all,"Atlas sync - Week 11 ").map((item)=>item.messageId)).toEqual(["w11","deck","starred"]);
    expect(meetingFiles(all,"Atlas review").map((item)=>item.messageId)).toEqual(["w12","deck","starred"]);
    // A single invitation, and files with none, are left alone.
    expect(meetingFiles([week10,deck],"Atlas review")).toEqual([week10,deck]);
    expect(meetingFiles([deck],"Atlas review")).toEqual([deck]);
  });
  it("names a reason once only when every row shares it", () => {
    expect(sharedReason([{reason:"Matching topic"},{reason:"Matching topic"}])).toBe("Matching topic");
    expect(sharedReason([{reason:"Matching topic"},{reason:"Matching topic and attendees"}])).toBeNull();
    expect(sharedReason([{reason:"Matching topic"},{}])).toBeNull();
    expect(sharedReason([])).toBeNull();
  });
});
