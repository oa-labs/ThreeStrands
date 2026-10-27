import { describe, expect, it } from "vitest";
import type { Message, MessageAttachment, MessageTrust, SenderDeliveryPattern, SenderFamiliarity, SenderRule, SplitInbox, ThreadDetail } from "./domain";
import { buildThreadTextIndex } from "./quotedHistory";
import { analyzeLinks, bulkMessage, latestInbound, matchingRule, matchingSplit, siteOf, trustHeadline, trustReport, TRUST_ATTACHMENT_LIMIT, TRUST_MISMATCH_LIMIT, TRUST_SUBJECT_WORD_LIMIT, type LinkAnalysis, type TrustInput } from "./senderSignals";

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

describe("matching mute rule", () => {
  const rule = (matchKind: SenderRule["matchKind"], matchValue: string, accountId = "you@example.com"): SenderRule =>
    ({ id: `${matchKind}:${matchValue}:${accountId}`, accountId, matchKind, matchValue, action: "mute", createdAt: "2026-10-14T00:00:00Z" });

  it("matches the exact address or exact domain in the same account only", () => {
    expect(matchingRule([rule("address", "news@launch.example")], "You@Example.com", "News@Launch.example")?.matchKind).toBe("address");
    expect(matchingRule([rule("domain", "launch.example")], "you@example.com", "billing@launch.example")?.matchKind).toBe("domain");
    expect(matchingRule([rule("domain", "launch.example")], "you@example.com", "news@mail.launch.example")).toBeNull();
    expect(matchingRule([rule("address", "news@launch.example")], "you@example.com", "breaking-news@launch.example")).toBeNull();
    expect(matchingRule([rule("address", "news@launch.example", "work@example.org")], "you@example.com", "news@launch.example")).toBeNull();
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
    expect(analyzeLinks(newsletter)).toMatchObject({ sites: ["launch.example", "social.example"], mismatches: [], trackingImages: 2 });
  });

  it("flags plain-paragraph link text that names one site while the link opens another", () => {
    const notice = `
      <div><p>Your account needs attention. Sign in at
        <a href="http://203.0.113.9/login">https://www.bank.example/login</a> or
        <a href="https://bank-example.test/verify">bank.example</a> today.</p>
        <p><a href="https://bank-example.test/again">BANK.EXAMPLE</a></p>
        <p><a href="https://help.bank.example">help.bank.example</a></p></div>`;
    const links = analyzeLinks(notice);
    expect(links.mismatches).toMatchObject([
      { shown: "bank.example", opens: "203.0.113.9", link: { href: "http://203.0.113.9/login", text: "https://www.bank.example/login" } },
      { shown: "bank.example", opens: "bank-example.test" },
    ]);
    expect(links.trackingImages).toBe(0);
  });

  it("reads links only from the part shown before folded quoted history", () => {
    // Someone else's mail system rewrote the links in the email being quoted:
    // the text still names the site, but the link opens a scanning service.
    const rewritten = '<a href="https://scan.relay.example/v2/url?u=https-3A__partner.example_">partner.example</a>';
    const replyAttribution = `
      <div dir="ltr"><p>Thanks, see you Tuesday.</p>
        <p><a href="https://agenda.example/tue">agenda.example/tue</a></p></div>
      <div><div dir="ltr">On Tue, Oct 13, 2026 at 9:14 AM Jane Doe &lt;jane@example.com&gt; wrote:<br></div>
        <blockquote style="margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex">
          <p>Details are at ${rewritten}.</p>
          <img src="https://open.tracker.example/q.gif" width="1" height="1" alt="">
        </blockquote></div>`;
    const headerCluster = `
      <table><tr><td><p>Approved — go ahead.</p></td></tr></table>
      <hr>
      <p><b>From:</b> Jane Doe &lt;jane@example.com&gt;<br><b>Sent:</b> Tuesday, October 13, 2026 9:14 AM<br>
        <b>To:</b> Bob Lee &lt;bob@example.com&gt;<br><b>Subject:</b> Order</p>
      <table><tr><td><p>Track the order at ${rewritten}.</p></td></tr></table>`;
    for (const body of [replyAttribution, headerCluster]) {
      const links = analyzeLinks(body);
      expect(links.mismatches).toEqual([]);
      expect(links.sites).not.toContain("relay.example");
    }
    expect(analyzeLinks(replyAttribution)).toMatchObject({ sites: ["agenda.example"], mismatches: [], trackingImages: 1 });

    // The same link in the reply's own words still warns, quoted history or not.
    const ownWords = `<p>Sign in at ${rewritten} today.</p>${replyAttribution}`;
    expect(analyzeLinks(ownWords).mismatches).toMatchObject([{ shown: "partner.example", opens: "relay.example" }]);
    expect(analyzeLinks(`<p>Sign in at ${rewritten} today.</p>`).mismatches).toMatchObject([{ shown: "partner.example", opens: "relay.example" }]);
  });

  it("folds with the conversation text before the message, as the reader does", () => {
    // A signature the thread already showed folds along with the quote below it.
    const signature = 'Jane Doe<br>Partnerships, Example Co<br><a href="https://scan.relay.example/r?u=partner.example">partner.example</a>';
    const body = `Fixed now.<br><br>${signature}<br><br>On Mon, A. Sender wrote:<blockquote>Is the feed fixed?</blockquote>`;
    const prior = buildThreadTextIndex(["Can you check the feed?\n\nJane Doe\nPartnerships, Example Co\npartner.example"]).before(1);
    expect(analyzeLinks(body, prior).mismatches).toEqual([]);
    expect(analyzeLinks(body).mismatches).toMatchObject([{ shown: "partner.example", opens: "relay.example" }]);
  });

  it("reads nothing from an empty body and never treats an email address as link text", () => {
    expect(analyzeLinks("  ")).toEqual({ sites: [], mismatches: [], links: [], trackingImages: 0 });
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
  const noLinks: LinkAnalysis = { sites: [], mismatches: [], links: [], trackingImages: 0 };
  const trust = (authentication: MessageTrust["authentication"], extra: Partial<MessageTrust> = {}): MessageTrust =>
    ({ fromDomain: "launch.example", authentication, replyTo: null, ...extra });
  const verdicts = (spf: string | null, dkim: string | null, dmarc: string | null, dkimDomains: string[] = [], spfDomains: string[] = []) =>
    ({ spf, dkim, dmarc, dkimDomains, spfDomains });
  const report = (value: MessageTrust | null, links = noLinks) =>
    trustReport({ trust: value, links, familiarity: null, sender: "Launch <news@launch.example>", subject: "Launch news", attachments: [], bulk: true });
  const first = (value: MessageTrust | null) => {
    const { rows, details } = report(value);
    return [...rows, ...details].find((finding) => finding.kind === "authentication");
  };

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

  it("verifies a sender without a DMARC result when SPF or DKIM passed for the From domain's organization", () => {
    // A relay that drops the DMARC result: the envelope matches From, the signature is the mail host's.
    expect(first(trust(verdicts("pass", "pass", null, ["tenant.mailhost.example"], ["launch.example"])))).toMatchObject({
      tone: "verified", title: "Sender verified", detail: "launch.example passed SPF for its own domain",
    });
    expect(first(trust(verdicts("softfail", "pass", null, ["mail.launch.example"])))).toMatchObject({
      tone: "verified", detail: "launch.example passed DKIM for its own domain",
    });
    expect(first(trust(verdicts("pass", "pass", null, ["launch.example"], ["bounce.launch.example"])))).toMatchObject({
      detail: "launch.example passed DKIM and SPF for its own domain",
    });
  });

  it("does not verify from passes for other domains, look-alikes, or over a DMARC result", () => {
    expect(first(trust(verdicts("pass", "pass", null, ["mailer.example"], ["bounces.mailer.example"])))).toMatchObject({ tone: "info", title: "Partly verified" });
    expect(first(trust(verdicts("pass", null, null, [], ["notlaunch.example"])))).toMatchObject({ tone: "info", title: "Partly verified" });
    expect(first(trust(verdicts("pass", "pass", "fail", ["launch.example"], ["launch.example"])))).toMatchObject({ tone: "warning", title: "Sender not verified" });
    expect(first(trust(verdicts("pass", "pass", "none", ["launch.example"], ["launch.example"])))).toMatchObject({ tone: "info", title: "Partly verified" });
    expect(first(trust(verdicts("pass", null, null, [], ["launch.example"]), { fromDomain: "" }))).toMatchObject({ tone: "info", title: "Partly verified" });
  });

  it("says when the server reported no checks, and lists nothing about headers it does not have", () => {
    expect(first(trust(null))).toMatchObject({ tone: "info", title: "No sender checks" });
    expect(first(trust(verdicts("none", "neutral", null)))).toMatchObject({ tone: "info", title: "No sender checks" });
    expect(report(null)).toEqual({ summary: null, rows: [], details: [] });
  });

  it("lists warnings as rows and folds the verdict, sites, and tracking into details", () => {
    const links: LinkAnalysis = {
      sites: ["launch.example", "social.example"],
      mismatches: Array.from({ length: TRUST_MISMATCH_LIMIT + 2 }, (_, index) => ({ shown: "bank.example", opens: `site${index}.example`, link: { href: `https://site${index}.example/`, text: "bank.example" } })),
      links: [],
      trackingImages: 1,
    };
    const { summary, rows, details } = report(trust(verdicts("pass", "pass", "pass"), { replyTo: "desk@elsewhere.example" }), links);
    expect(summary).toBe("careful");
    expect(rows.map((finding) => finding.kind)).toEqual(["replyTo", "mismatch", "mismatch", "mismatch"]);
    expect(rows[3].detail).toBe("Shows bank.example, opens site2.example (and 2 more)");
    expect(rows[1].link).toEqual({ href: "https://site0.example/", text: "bank.example" });
    expect(details.map((finding) => finding.kind)).toEqual(["authentication", "sites", "tracking"]);
    expect(details[1]).toMatchObject({ title: "Links open 2 sites", detail: "launch.example, social.example" });
    expect(details[2].title).toBe("1 tracking image");
  });

  it("makes the sender check the headline when nothing warns and there is no history", () => {
    const { summary, rows, details } = report(trust(verdicts("pass", "pass", "pass")), { ...noLinks, sites: ["launch.example"] });
    expect(summary).toBeNull();
    expect(rows.map((finding) => finding.title)).toEqual(["Sender verified"]);
    expect(details.map((finding) => finding.kind)).toEqual(["sites"]);
  });
});

describe("trust signals from the user's own history", () => {
  const verified: MessageTrust = { fromDomain: "partner.example", authentication: { spf: "pass", dkim: "pass", dmarc: "pass", dkimDomains: ["partner.example"], spfDomains: ["partner.example"] }, replyTo: null };
  const stranger: SenderFamiliarity = { sentCount: 0, firstSentAt: null, receivedBefore: false, contactName: null, nameMatches: [], knownDomains: ["partner.example", "studio.example", "example.co.uk"] };
  const input = (overrides: Partial<TrustInput> = {}): TrustInput => ({
    trust: verified,
    links: analyzeLinks(""),
    familiarity: stranger,
    sender: "Jane Rivera <jane@partner.example>",
    subject: "Studio schedule",
    attachments: [],
    bulk: false,
    ...overrides,
  });
  const titles = (value: TrustInput) => trustReport(value).rows.map((finding) => finding.title);
  const warnings = (value: TrustInput) => trustReport(value).rows.filter((finding) => finding.tone === "warning");
  const attachment = (filename: string, extra: Partial<MessageAttachment> = {}): MessageAttachment => ({ id: filename, filename, mimeType: "application/octet-stream", size: 1, ...extra });

  const confirmedPattern: SenderDeliveryPattern = {
    id: "confirmed-pattern", senderEmail: "requests@signing.example", replyTo: "ana@customer.example",
    linkOrigins: ["https://help.signing.example", "https://secure.signing.example"], authenticated: true,
    sourceMessageId: "confirmed-request", confirmedAt: "2026-10-26T12:00:00Z",
  };
  const deliveryInput = (overrides: Partial<TrustInput> = {}): TrustInput => input({
    messageId: "future-request", sender: "Requests <requests@signing.example>",
    trust: { ...verified, fromDomain: "signing.example", replyTo: "ana@customer.example" },
    links: analyzeLinks('<p><a href="https://secure.signing.example/sign?token=private">Review</a></p><footer><a href="https://help.signing.example/faq">Help</a></footer>'),
    familiarity: { ...stranger, knownDomains: [], deliveryPatterns: [confirmedPattern] },
    ...overrides,
  });

  it("uses explicit feedback for the exact requester, sender, and web origins without calling the service a known contact", () => {
    const report = trustReport(deliveryInput());
    expect(report.summary).toBeNull();
    expect(report.rows).toMatchObject([{ kind: "delivery", tone: "info", title: "Matches a request you confirmed" }]);
    expect(report.details.find((finding) => finding.kind === "replyTo")).toMatchObject({ tone: "info" });
    expect(report.feedback?.linkOrigins).toEqual(confirmedPattern.linkOrigins);
    expect(JSON.stringify(report.feedback)).not.toContain("private");
    expect(trustReport(deliveryInput({ messageId: "confirmed-request" })).rows[0].title).toBe("You confirmed this email is legitimate");

    // A structurally different portal notification uses the same general rule.
    const portal = { ...confirmedPattern, senderEmail: "notices@portal.example", linkOrigins: ["https://portal.example"], replyTo: "lee@studio.example" };
    const value = deliveryInput({ sender: portal.senderEmail, trust: { ...verified, fromDomain: "portal.example", replyTo: portal.replyTo }, links: analyzeLinks('<table><tr><td><a href="https://portal.example/jobs/123">Open job</a></td></tr></table>'), familiarity: { ...stranger, knownDomains: [], deliveryPatterns: [portal] } });
    expect(warnings(value)).toEqual([]);
    expect(trustReport(value).rows[0].title).toBe("Matches a request you confirmed");
  });

  it("calls out a new sending address, scheme, port, subdomain, or link destination for a confirmed requester", () => {
    for (const value of [
      deliveryInput({ sender: "other@signing.example" }),
      deliveryInput({ sender: "requests@new-service.example", trust: { ...verified, fromDomain: "new-service.example", replyTo: confirmedPattern.replyTo } }),
      ...["http://secure.signing.example", "https://secure.signing.example:8443", "https://other.signing.example", "https://unfamiliar.example"].map((origin) => deliveryInput({ links: analyzeLinks(`<a href="${origin}/sign">Review</a><a href="https://help.signing.example/faq">Help</a>`) })),
      deliveryInput({ links: analyzeLinks('<a href="https://secure.signing.example/sign">Review</a>') }),
    ]) {
      expect(warnings(value).map((finding) => finding.title)).toContain("Different delivery pattern");
      expect(trustReport(value).rows.some((finding) => finding.key === "delivery-confirmed")).toBe(false);
    }
  });

  it("does not recognize another person at the same organization or an unconfirmed service", () => {
    const other = deliveryInput({ trust: { ...verified, fromDomain: "signing.example", replyTo: "bob@customer.example" } });
    expect(warnings(other).map((finding) => finding.title)).toContain("Replies go elsewhere");
    expect(trustReport(other).rows.some((finding) => finding.key === "delivery-confirmed")).toBe(false);
    for (const familiarity of [null, { ...stranger, receivedBefore: true, knownDomains: [], deliveryPatterns: [] }]) {
      expect(warnings(deliveryInput({ familiarity })).map((finding) => finding.title)).toContain("Replies go elsewhere");
    }
  });

  it("answers a borrowed name only for the confirmed message or the same authenticated delivery", () => {
    const borrowed = (overrides: Partial<TrustInput> = {}) => deliveryInput({
      sender: "Ana Ortiz <requests@signing.example>",
      familiarity: { ...stranger, knownDomains: [], nameMatches: ["ana@customer.example"], deliveryPatterns: [confirmedPattern] },
      ...overrides,
    });
    const named = (value: TrustInput) => warnings(value).some((finding) => finding.key === "impersonation");
    expect(named(borrowed())).toBe(false);
    expect(named(borrowed({ messageId: "confirmed-request", familiarity: { ...stranger, knownDomains: [], nameMatches: ["ana@customer.example"], deliveryPatterns: [{ ...confirmedPattern, authenticated: false }] } }))).toBe(false);

    // A structurally different delivery: no Reply-To, one link origin.
    const newsletter = { ...confirmedPattern, senderEmail: "hello@studio.example", replyTo: null, linkOrigins: ["https://studio.example"] };
    const plain = (overrides: Partial<TrustInput> = {}) => input({
      messageId: "later", sender: "Jane Rivera <hello@studio.example>",
      trust: { ...verified, fromDomain: "studio.example" },
      links: analyzeLinks('<table><tr><td><a href="https://studio.example/news">Read</a></td></tr></table>'),
      familiarity: { ...stranger, nameMatches: ["jane@partner.example"], deliveryPatterns: [newsletter] },
      ...overrides,
    });
    expect(named(plain())).toBe(false);
    expect(trustReport(plain()).summary).toBeNull();

    for (const value of [
      plain({ familiarity: { ...stranger, nameMatches: ["jane@partner.example"], deliveryPatterns: [] } }),
      plain({ familiarity: { ...stranger, nameMatches: ["jane@partner.example"], deliveryPatterns: [{ ...newsletter, authenticated: false }] } }),
      plain({ trust: { ...verified, fromDomain: "studio.example", authentication: { ...verified.authentication!, dmarc: "fail" } } }),
      plain({ sender: "Jane Rivera <hi@studio.example>" }),
      plain({ links: analyzeLinks('<a href="https://other.example/pay">Read</a>') }),
    ]) {
      expect(named(value)).toBe(true);
    }
  });

  it("requires DMARC on both the confirmed request and the current message and preserves every other warning", () => {
    for (const dmarc of ["fail", "none", null]) {
      const value = deliveryInput({ trust: { ...verified, fromDomain: "signing.example", replyTo: confirmedPattern.replyTo, authentication: { ...verified.authentication!, dmarc } } });
      expect(warnings(value).map((finding) => finding.title)).toContain("Replies go elsewhere");
      expect(trustReport(value).rows.some((finding) => finding.key === "delivery-confirmed")).toBe(false);
    }
    expect(warnings(deliveryInput({ familiarity: { ...stranger, knownDomains: [], deliveryPatterns: [{ ...confirmedPattern, authenticated: false }] } })).map((finding) => finding.title)).toContain("Replies go elsewhere");
    for (const value of [
      deliveryInput({ subject: "COMPLETE REVlEW" }),
      deliveryInput({ attachments: [attachment("installer.exe")] }),
      deliveryInput({ links: analyzeLinks('<a href="https://secure.signing.example/sign">bank.example</a><a href="https://help.signing.example/faq">Help</a>') }),
      deliveryInput({ sender: "ana@customer.example <requests@signing.example>" }),
    ]) {
      expect(trustReport(value).summary).toBe("careful");
      expect(warnings(value).length).toBeGreaterThan(0);
    }
  });

  it("never reuses a pattern when distinct link analysis reaches its limit", () => {
    for (const count of [499, 500, 501]) {
      const links = analyzeLinks(Array.from({ length: count }, (_, index) => `<a href="https://secure.signing.example/${index}">Review</a>`).join(""));
      const value = deliveryInput({ links, familiarity: { ...stranger, knownDomains: [], deliveryPatterns: [{ ...confirmedPattern, linkOrigins: ["https://secure.signing.example"] }] } });
      expect(trustReport(value).feedback !== undefined).toBe(count < 500);
      expect(warnings(value).some((finding) => finding.kind === "replyTo")).toBe(count >= 500);
    }
  });

  it("calls someone the user writes to a known sender, and says how often and since when", () => {
    const report = trustReport(input({ familiarity: { ...stranger, sentCount: 12, firstSentAt: "2024-03-05T10:00:00Z" } }));
    expect(report.summary).toBe("known");
    expect(report.rows).toMatchObject([{ tone: "verified", kind: "history", title: "You've written to Jane 12 times", detail: "Since Mar 2024" }]);
    expect(report.details.map((finding) => finding.kind)).toEqual(["authentication"]);
    expect(titles(input({ familiarity: { ...stranger, sentCount: 1, firstSentAt: "2024-03-05T10:00:00Z" } }))).toEqual(["You've written to Jane once"]);
    expect(titles(input({ familiarity: { ...stranger, contactName: "Jane R." } }))).toEqual(["Saved in your contacts"]);
  });

  it("needs a DMARC pass before calling anyone known", () => {
    const familiar = { ...stranger, sentCount: 3 };
    expect(trustReport(input({ familiarity: familiar, trust: { ...verified, authentication: null } })).summary).toBeNull();
    expect(trustReport(input({ familiarity: familiar, trust: null })).summary).toBeNull();
    const partly = { spf: "pass", dkim: "pass", dmarc: null, dkimDomains: ["mailer.example"], spfDomains: ["mailer.example"] };
    expect(trustReport(input({ familiarity: familiar, trust: { ...verified, authentication: partly } })).summary).toBeNull();
  });

  it("calls someone the user writes to known when a relay dropped DMARC but SPF passed for their domain", () => {
    const relayed = { spf: "pass", dkim: "pass", dmarc: null, dkimDomains: ["tenant.mailhost.example"], spfDomains: ["partner.example"] };
    const report = trustReport(input({ familiarity: { ...stranger, sentCount: 22 }, trust: { ...verified, authentication: relayed } }));
    expect(report.summary).toBe("known");
    expect(report.details[0]).toMatchObject({ tone: "verified", title: "Sender verified" });
  });

  it("notes a first email from personal senders only, and says nothing when the history cannot be read", () => {
    expect(trustReport(input()).rows).toMatchObject([{ tone: "info", title: "First email from this address", detail: "jane@partner.example" }]);
    expect(trustReport(input()).summary).toBeNull();
    expect(titles(input({ familiarity: { ...stranger, receivedBefore: true } }))).toEqual(["Sender verified"]);
    expect(titles(input({ bulk: true }))).toEqual(["Sender verified"]);
    expect(titles(input({ familiarity: null }))).toEqual(["Sender verified"]);
  });

  it("warns when a display name the user knows arrives from a new address", () => {
    const value = input({ sender: "Jane Rivera <jane.rivera.office@freemail.example>", familiarity: { ...stranger, nameMatches: ["jane@partner.example", "j.rivera@partner.example"] } });
    const report = trustReport(value);
    expect(report.summary).toBe("careful");
    expect(report.rows[0]).toMatchObject({
      tone: "warning", kind: "impersonation", title: "Familiar name, different email address",
      detail: "Jane Rivera · Sending address: jane.rivera.office@freemail.example · Previously seen: jane@partner.example and 1 more",
      addressComparison: { name: "Jane Rivera", sending: "jane.rivera.office@freemail.example", previous: ["jane@partner.example", "j.rivera@partner.example"] },
    });
    // Someone the user writes to at this address is the person, not an impostor.
    expect(warnings({ ...value, familiarity: { ...stranger, sentCount: 1, nameMatches: ["jane@partner.example"] } })).toEqual([]);
  });

  it("warns when the subject writes words with look-alike letters, naming each trick once", () => {
    const report = trustReport(input({ subject: "Payouts Paused - COMPLETE REVlEW TO REClEVE PAYOUTS" }));
    expect(report.summary).toBe("careful");
    expect(report.rows[0]).toMatchObject({
      tone: "warning", kind: "subject", title: "Subject uses look-alike letters",
      detail: "“REVlEW” and “REClEVE” use a lowercase l in place of I",
    });
    expect(warnings(input({ subject: "Your pаypal ACC0UNT is on hold" }))[0]).toMatchObject({
      detail: "“pаypal” uses a Cyrillic letter in place of a Latin one; “ACC0UNT” uses the digit 0 in place of O",
    });
  });

  it("names a few disguised subject words and counts the rest", () => {
    const detail = warnings(input({ subject: "VERlFY ACCOUNT ClAIM PRlZE NOW BEFORE lT FAlLS TODAY" }))[0].detail;
    expect(detail).toBe(`“VERlFY”, “ClAIM”, and “PRlZE” use a lowercase l in place of I (and 1 more)`);
    expect(TRUST_SUBJECT_WORD_LIMIT).toBe(3);
  });

  it("says nothing about ordinary subjects, even from someone the user writes to", () => {
    for (const subject of ["Q3 OKRs and the iPhone launch", "Re: COVID-19 update", "Москва: встреча в среду", "", "Fwd: MP3s from Win10Pro"]) {
      expect(warnings(input({ subject })), subject).toEqual([]);
    }
    expect(warnings(input({ subject: "PLEASE REVlEW", familiarity: { ...stranger, sentCount: 4 } }))).toHaveLength(1);
  });

  it("warns when a display name spells out another organization's address or domain", () => {
    expect(warnings(input({ sender: '"support@bank.example" <help@other.example>' }))[0]).toMatchObject({ title: "Name shows support@bank.example", detail: "Sent from help@other.example" });
    expect(warnings(input({ sender: "Shop.net Orders <orders@mailer.example>" }))[0]).toMatchObject({ title: "Name shows Shop.net" });
    // The sender's own organization, under any suffix, and initials are not claims.
    expect(warnings(input({ sender: "Partner.com <news@partner.example>" }))).toEqual([]);
    expect(warnings(input({ sender: "support@partner.example <help@mail.partner.example>" }))).toEqual([]);
    expect(warnings(input({ sender: "J.R. Doe <jr@other.example>" }))).toEqual([]);
    expect(warnings(input({ sender: "Jo.Doe <jo@other.example>" }))).toEqual([]);
  });

  it("warns when the sender's domain imitates one the user corresponds with", () => {
    for (const domain of ["partn3r.example", "xn--prtner-3nf.example", "parnter.example", "partnerr.example", "exampl.co.uk"]) {
      const [warning] = warnings(input({ sender: `Jane <jane@${domain}>`, trust: { ...verified, fromDomain: domain } }));
      expect(warning, domain).toMatchObject({ kind: "lookalike" });
      expect(warning.title, domain).toMatch(/^Address looks like (partner\.example|example\.co\.uk)$/);
    }
    const [punycode] = warnings(input({ sender: "Jane <jane@xn--prtner-3nf.example>" }));
    expect(punycode.detail).toBe("Sent from pаrtner.example (xn--prtner-3nf.example)");
    // A subdomain of a known site, a different suffix, or a short name a letter away is not a look-alike.
    expect(warnings(input({ sender: "Jane <jane@mail.partner.example>" }))).toEqual([]);
    expect(warnings(input({ sender: "Jane <jane@partner.test>" }))).toEqual([]);
    expect(warnings(input({ sender: "Ann <ann@stdio.example>", familiarity: { ...stranger, knownDomains: ["stdo.example"] } }))).toEqual([]);
  });

  it("warns about an address that mixes alphabets even with no known site to compare", () => {
    const [warning] = warnings(input({ sender: "Shop <orders@xn--shp-ted.example>", familiarity: { ...stranger, knownDomains: [] } }));
    expect(warning).toMatchObject({ title: "Address uses look-alike letters", detail: "Sent from shоp.example (xn--shp-ted.example)" });
    // A name written wholly in another alphabet is just a name.
    expect(warnings(input({ sender: "Магазин <shop@xn--80aairftm.example>", familiarity: { ...stranger, knownDomains: [] } }))).toEqual([]);
  });

  it("warns once per link about addresses that hide where they go, so each can be shown", () => {
    const html = `
      <p><a href="https://partner.example@login.other.example/x">Sign in</a>
        <a href="https://partner.example@login.other.example/y">Again</a></p>
      <p><a href="https://partnr.example/pay">Pay now</a></p>
      <table><tr><td><a href="https://partner.example.secure.other.test/">Review</a></td></tr></table>
      <div><a href="http://198.51.100.7/">Download</a></div>
      <a href="https://studio.example/ok">Studio</a>`;
    const rows = warnings(input({ links: analyzeLinks(html) }));
    expect(rows.map((finding) => [finding.title, finding.detail])).toEqual([
      ["Link hides where it goes", "Shows partner.example, opens other.example"],
      ["Link looks like partner.example", "Opens partnr.example"],
      ["Link looks like partner.example", "Opens partner.example.secure.other.test (and 1 more)"],
    ]);
    expect(rows[0].link).toEqual({ href: "https://partner.example@login.other.example/x", text: "Sign in" });
    expect(warnings(input({ links: analyzeLinks('<a href="http://198.51.100.7/">Download</a>') }))[0]).toMatchObject({ title: "Link opens a numeric address", detail: "Opens 198.51.100.7" });
    expect(warnings(input({ links: analyzeLinks('<a href="https://xn--shp-ted.example/">Shop</a>'), familiarity: null }))[0]).toMatchObject({ title: "Link uses look-alike letters" });
  });

  it("does not add a second warning to a link whose text already mismatches", () => {
    const rows = warnings(input({ links: analyzeLinks('<a href="http://198.51.100.7/">partner.example</a>') }));
    expect(rows.map((finding) => finding.kind)).toEqual(["mismatch"]);
  });

  it("warns about programs and disguised files from anyone, and other risky files only from strangers", () => {
    const files = [attachment("invoice.pdf.exe"), attachment("setup.exe"), attachment("budget.xlsm"), attachment("logo.png", { inline: true }), attachment("notes.txt")];
    expect(warnings(input({ attachments: files })).map((finding) => finding.title)).toEqual(["invoice.pdf.exe", "setup.exe", "budget.xlsm"]);
    expect(warnings(input({ attachments: files, familiarity: { ...stranger, sentCount: 4 } })).map((finding) => finding.title)).toEqual(["invoice.pdf.exe", "setup.exe"]);
    const many = Array.from({ length: TRUST_ATTACHMENT_LIMIT + 2 }, (_, index) => attachment(`tool${index}.exe`));
    expect(warnings(input({ attachments: many }))).toHaveLength(TRUST_ATTACHMENT_LIMIT);
  });

  it("marks borrowed names, look-alike domains, and risky files strong enough to head the message", () => {
    const strongKeys = (value: TrustInput) => warnings(value).filter((finding) => finding.strong).map((finding) => finding.kind);
    expect(strongKeys(input({ sender: "Jane Rivera <jr@freemail.example>", familiarity: { ...stranger, nameMatches: ["jane@partner.example"] } }))).toEqual(["impersonation"]);
    expect(strongKeys(input({ sender: '"billing@studio.example" <desk@other.example>' }))).toEqual(["impersonation"]);
    expect(strongKeys(input({ sender: "Jane <jane@partn3r.example>", trust: { ...verified, fromDomain: "partn3r.example" } }))).toEqual(["lookalike"]);
    expect(strongKeys(input({ links: analyzeLinks('<a href="https://partnr.example/pay">Pay</a>') }))).toEqual(["link"]);
    expect(strongKeys(input({ links: analyzeLinks('<a href="https://xn--shp-ted.example/">Shop</a>'), familiarity: null }))).toEqual(["link"]);
    expect(strongKeys(input({ attachments: [attachment("invoice.pdf.exe")] }))).toEqual(["attachment"]);
  });

  it("keeps weaker warnings off the message body", () => {
    const weak = warnings(input({
      subject: "COMPLETE REVlEW",
      trust: { ...verified, replyTo: "desk@elsewhere.example" },
      links: analyzeLinks('<a href="http://198.51.100.7/">partner.example</a><a href="https://partner.example@other.example/">Sign in</a>'),
    }));
    expect(weak.map((finding) => finding.kind)).toEqual(["subject", "replyTo", "mismatch", "link"]);
    expect(weak.some((finding) => finding.strong)).toBe(false);
    const failed = warnings(input({ trust: { ...verified, authentication: { ...verified.authentication!, dmarc: "fail" } } }));
    expect(failed).toMatchObject([{ kind: "authentication" }]);
    expect(failed[0].strong).toBeUndefined();
  });

  it("sums up a report for the reader header as its first warning and a count of the rest", () => {
    expect(trustHeadline(trustReport(input({ familiarity: { ...stranger, sentCount: 3 } })))).toBeNull();
    expect(trustHeadline(trustReport(input({ attachments: [attachment("setup.exe")] })))).toBe("setup.exe");
    expect(trustHeadline(trustReport(input({ subject: "COMPLETE REVlEW", attachments: [attachment("setup.exe"), attachment("tool.exe")] }))))
      .toBe("Subject uses look-alike letters · 2 more");
  });

  it("keeps a Reply-To at a domain the user writes to out of the warnings", () => {
    const familiar = trustReport(input({ trust: { ...verified, replyTo: "desk@studio.example" } }));
    expect(familiar.rows.some((finding) => finding.kind === "replyTo")).toBe(false);
    expect(familiar.details.find((finding) => finding.kind === "replyTo")).toMatchObject({ tone: "info", detail: "Reply-To is desk@studio.example, a domain you write to" });
    expect(warnings(input({ trust: { ...verified, replyTo: "desk@elsewhere.example" } }))[0]).toMatchObject({ kind: "replyTo" });
  });
});
