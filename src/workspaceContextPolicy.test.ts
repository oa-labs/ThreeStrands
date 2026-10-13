import { describe, expect, it } from "vitest";
import { contextQuery, contextTerms, rankContext } from "./workspaceContextPolicy";
import type { WorkspaceConversation } from "./domain";

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
});
