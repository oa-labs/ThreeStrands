import { describe, expect, it } from "vitest";
import type { Message, MessageTrust, SenderHistory, SplitInbox, ThreadDetail } from "./domain";
import { analyzeLinks, bulkMessage, describeSender, latestInbound, matchingSplit, siteOf, trustFindings, TRUST_MISMATCH_LIMIT, type LinkAnalysis } from "./senderSignals";

const own = new Set(["you@example.com"]);

function message(id: string, sender: string, extra: Partial<Message> = {}): Message {
  return { id, threadId: "t", sender, recipients: [], sentAt: "2026-10-01T00:00:00Z", bodyHtml: "", bodyText: "", unread: false, attachments: [], ...extra };
}

function detail(messages: Message[], labels: string[] = ["INBOX"]): ThreadDetail {
  return { thread: { id: "t", labels } as ThreadDetail["thread"], messages };
}

describe("bulk mail detection", () => {
  const listed = message("n", "News <news@launch.example>", { unsubscribe: { methods: ["web"], listId: null } });

  it("counts a List-Unsubscribe option or the Updates and Promotions categories", () => {
    expect(bulkMessage(detail([listed]), own)?.id).toBe("n");
    const plain = message("p", "news@launch.example");
    expect(bulkMessage(detail([plain], ["INBOX", "CATEGORY_UPDATES"]), own)?.id).toBe("p");
    expect(bulkMessage(detail([plain], ["INBOX", "CATEGORY_PROMOTIONS"]), own)?.id).toBe("p");
    expect(bulkMessage(detail([plain], ["INBOX", "CATEGORY_SOCIAL", "CATEGORY_PERSONAL"]), own)).toBeNull();
    expect(bulkMessage(detail([message("e", "news@launch.example", { unsubscribe: { methods: [], listId: "x" } })]), own)).toBeNull();
  });

  it("looks at the newest message from someone else, past the user's own reply", () => {
    const reply = message("r", "You <you@example.com>");
    expect(latestInbound(detail([listed, reply]), own)?.id).toBe("n");
    expect(bulkMessage(detail([listed, reply]), own)?.id).toBe("n");
    expect(latestInbound(detail([reply]), own)).toBeNull();
    expect(bulkMessage(detail([reply], ["CATEGORY_UPDATES"]), own)).toBeNull();
  });
});

describe("sender facts", () => {
  const now = new Date("2026-10-14T12:00:00Z");
  const history = (overrides: Partial<SenderHistory>): SenderHistory => ({
    receivedCount: 0, readCount: 0, firstAt: null, lastSentAt: null, recentReceivedAt: [], inboxThreadIds: [], threads: [], ...overrides,
  });

  it("states volume, cadence, how much is read, and the user's latest message", () => {
    const weekly = ["2026-10-07", "2026-09-30", "2026-09-23", "2026-09-16", "2026-09-09"].map((day) => `${day}T09:00:00Z`);
    const facts = describeSender(history({ receivedCount: 11, readCount: 2, firstAt: "2026-06-02T09:00:00Z", recentReceivedAt: weekly, lastSentAt: "2026-09-01T00:00:00Z" }), now);
    expect(facts[0]).toMatch(/^11 emails since /);
    expect(facts.slice(1)).toEqual(["Weekly", "2 of 11 read", expect.stringMatching(/^You last wrote /)]);
  });

  it("says none or all are read, and skips the read count for a single email", () => {
    expect(describeSender(history({ receivedCount: 3, readCount: 0, firstAt: "2026-06-02T09:00:00Z" }), now)).toContain("None read");
    expect(describeSender(history({ receivedCount: 3, readCount: 3, firstAt: "2026-06-02T09:00:00Z" }), now)).toContain("All read");
    const single = describeSender(history({ receivedCount: 1, readCount: 1, firstAt: "2026-06-02T09:00:00Z" }), now);
    expect(single).toHaveLength(1);
    expect(single[0]).toMatch(/^1 email · /);
    expect(describeSender(history({}), now)).toEqual([]);
  });
});

describe("matching split", () => {
  const split = (matchKind: SplitInbox["matchKind"], matchValue: string, accountId = "you@example.com"): SplitInbox =>
    ({ id: `${matchKind}:${matchValue}:${accountId}`, name: matchValue, matchKind, matchValue, sortOrder: 0, createdAt: "", accountId });

  it("finds an address or domain rule in the same account only", () => {
    expect(matchingSplit([split("pattern", "news@")], "you@example.com", "News@Launch.example")?.matchValue).toBe("news@");
    expect(matchingSplit([split("domain", "launch.example")], "you@example.com", "news@launch.example")?.matchValue).toBe("launch.example");
    expect(matchingSplit([split("domain", "launch.example", "work@example.org")], "you@example.com", "news@launch.example")).toBeNull();
    expect(matchingSplit([split("domain", "example")], "you@example.com", "news@launch.example")).toBeNull();
    expect(matchingSplit([split("label", "CATEGORY_UPDATES")], "you@example.com", "news@launch.example")).toBeNull();
  });
});

describe("link analysis", () => {
  it("groups hosts into sites, keeping country domains that register one level down", () => {
    expect(siteOf("WWW.Example.com.")).toBe("example.com");
    expect(siteOf("click.mail.example.com")).toBe("example.com");
    expect(siteOf("shop.example.co.uk")).toBe("example.co.uk");
    expect(siteOf("news.example.de")).toBe("example.de");
    expect(siteOf("192.0.2.10")).toBe("192.0.2.10");
  });

  it("reads a table-laid-out newsletter: sites by frequency, tracking pixels, and matching link text", () => {
    const newsletter = `
      <table role="presentation"><tr><td>
        <a href="https://click.launch.example/r/1">Read the post</a>
        <a href="https://www.launch.example/pricing">launch.example/pricing</a>
        <a href="https://click.launch.example/r/2"><img src="https://cdn.launch.example/hero.png" width="600" height="300"></a>
        <a href="https://social.example/launch">Follow us</a>
        <a href="mailto:hello@launch.example">hello@launch.example</a>
        <a href="javascript:void(0)">launch.example</a>
      </td></tr></table>
      <img src="https://open.tracker.example/o.gif" width="1" height="1" alt="">
      <img src="https://open.tracker.example/b.gif" style="display:none">
      <img src="cid:logo" width="1" height="1">`;
    expect(analyzeLinks(newsletter)).toEqual({ sites: ["launch.example", "social.example"], mismatches: [], trackingImages: 2 });
  });

  it("flags plain-paragraph link text that names one site while the link opens another", () => {
    const notice = `
      <div><p>Your account needs attention. Sign in at
        <a href="http://203.0.113.9/login">https://www.bank.example/login</a> or
        <a href="https://bank-example.test/verify">bank.example</a> today.</p>
        <p><a href="https://bank-example.test/again">BANK.EXAMPLE</a></p>
        <p><a href="https://help.bank.example">help.bank.example</a></p></div>`;
    const links = analyzeLinks(notice);
    expect(links.mismatches).toEqual([
      { shown: "bank.example", opens: "203.0.113.9" },
      { shown: "bank.example", opens: "bank-example.test" },
    ]);
    expect(links.trackingImages).toBe(0);
  });

  it("reads nothing from an empty body and never treats an email address as link text", () => {
    expect(analyzeLinks("  ")).toEqual({ sites: [], mismatches: [], trackingImages: 0 });
    expect(analyzeLinks('<a href="https://other.example">ops@bank.example</a>').mismatches).toEqual([]);
  });

  it("does not fetch or run anything while reading the body", () => {
    // A parsed document is inert: no script runs and no image request starts.
    (globalThis as { __ran?: boolean }).__ran = false;
    analyzeLinks('<img src="https://tracker.example/p.gif" onerror="globalThis.__ran=true"><script>globalThis.__ran=true</script>');
    expect((globalThis as { __ran?: boolean }).__ran).toBe(false);
  });
});

describe("trust findings", () => {
  const noLinks: LinkAnalysis = { sites: [], mismatches: [], trackingImages: 0 };
  const trust = (authentication: MessageTrust["authentication"], extra: Partial<MessageTrust> = {}): MessageTrust =>
    ({ fromDomain: "launch.example", authentication, replyTo: null, ...extra });
  const verdicts = (spf: string | null, dkim: string | null, dmarc: string | null, dkimDomains: string[] = []) => ({ spf, dkim, dmarc, dkimDomains });
  const first = (value: MessageTrust | null) => trustFindings(value, noLinks)[0];

  it("calls a DMARC pass verified and names every check that passed", () => {
    expect(first(trust(verdicts("pass", "pass", "pass")))).toMatchObject({ tone: "verified", title: "Sender verified", detail: "launch.example passed DMARC, DKIM, and SPF" });
    expect(first(trust(verdicts("fail", null, "pass")))).toMatchObject({ detail: "launch.example passed DMARC" });
  });

  it("warns on a DMARC failure, or when SPF and DKIM both fall short without DMARC", () => {
    expect(first(trust(verdicts("pass", "pass", "fail")))).toMatchObject({ tone: "warning", title: "Sender not verified", detail: "launch.example failed DMARC" });
    expect(first(trust(verdicts("softfail", "fail", null)))).toMatchObject({ tone: "warning", detail: "Neither SPF nor DKIM passed for launch.example" });
  });

  it("calls partial checks partly verified and names a signer from another organization", () => {
    expect(first(trust(verdicts("pass", "pass", "none", ["mailer.example", "mail.launch.example"])))).toMatchObject({
      tone: "info", title: "Partly verified", detail: "DKIM and SPF passed, no DMARC result; signed by mailer.example",
    });
    expect(first(trust(verdicts("pass", null, null)))).toMatchObject({ detail: "SPF passed, no DMARC result" });
  });

  it("says when the server reported no checks, and lists nothing about headers it does not have", () => {
    expect(first(trust(null))).toMatchObject({ tone: "info", title: "No sender checks" });
    expect(first(trust(verdicts("none", "neutral", null)))).toMatchObject({ tone: "info", title: "No sender checks" });
    expect(trustFindings(null, noLinks)).toEqual([]);
  });

  it("lists warnings first, then the verdict, sites, and tracking", () => {
    const links: LinkAnalysis = {
      sites: ["launch.example", "social.example"],
      mismatches: Array.from({ length: TRUST_MISMATCH_LIMIT + 2 }, (_, index) => ({ shown: "bank.example", opens: `site${index}.example` })),
      trackingImages: 1,
    };
    const findings = trustFindings(trust(verdicts("pass", "pass", "pass"), { replyTo: "desk@elsewhere.example" }), links);
    expect(findings.map((finding) => finding.kind)).toEqual(["replyTo", "mismatch", "mismatch", "mismatch", "authentication", "sites", "tracking"]);
    expect(findings[3].detail).toBe("Shows bank.example, opens site2.example (and 2 more)");
    expect(findings[5]).toMatchObject({ title: "Links open 2 sites", detail: "launch.example, social.example" });
    expect(findings[6].title).toBe("1 tracking image");
  });
});
