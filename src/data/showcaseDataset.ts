import type { DemoDataset, DemoMessageSeed } from "./demoDataset";
import type { Goal, ScheduleEvent, Thread, ThreadTask } from "../domain";
import { periodFor } from "../goals";

/**
 * Marketing-screenshot data: a believable two-account mailbox for a fictional
 * person. Every name, company, and address is invented and every domain is
 * reserved (`.example`, RFC 2606), so a screenshot can never expose real
 * correspondence. Timestamps are relative to `now` so "2h ago" and "today"
 * stay fresh; pass a fixed `now` for reproducible captures.
 */

export const SHOWCASE_WORK_ACCOUNT = "maya@harborlight.example";
export const SHOWCASE_PERSONAL_ACCOUNT = "maya.chen@example.com";

const me = { work: `Maya Chen <${SHOWCASE_WORK_ACCOUNT}>`, personal: `Maya Chen <${SHOWCASE_PERSONAL_ACCOUNT}>` };
const people = {
  priya: "Priya Natarajan <priya@harborlight.example>",
  theo: "Theo Alvarez <theo@harborlight.example>",
  jordan: "Jordan Blake <jordan@harborlight.example>",
  elena: "Elena Rossi <elena@harborlight.example>",
  daniel: "Daniel Kim <daniel@harborlight.example>",
  aisha: "Aisha Rahman <aisha@harborlight.example>",
  sam: "Sam Okafor <sam@harborlight.example>",
  marcus: "Marcus Webb <marcus@brightwater.example>",
  builds: "Harborlight CI <notifications@harborlight.example>",
  hosting: "Cloudline Billing <billing@cloudline.example>",
  metrics: "Pulse Weekly <digest@pulsemetrics.example>",
  lena: "Lena Ortiz <lena.ortiz@example.net>",
  ben: "Ben Chen <ben.chen@example.org>",
  airline: "Skyward Air <itinerary@skyward.example>",
  reader: "The Weekend Reader <issues@weekendreader.example>",
  library: "Riverside Public Library <holds@riversidelibrary.example>",
  prize: "Prize Center <winner@prizes.example>",
};

type ThreadSeed = {
  id: string;
  accountId: string;
  subject: string;
  labels: string[];
  unread?: boolean;
  starred?: boolean;
  archived?: boolean;
  trashed?: boolean;
  summary?: string;
  /** Oldest first; `ago` is minutes before `now`. */
  messages: (Omit<DemoMessageSeed, "sentAt"> & { ago: number })[];
};

function paragraphs(...lines: string[]) {
  return lines.map((line) => `<p>${line}</p>`).join("\n");
}

function plain(html: string) {
  return html.replace(/<[^>]+>/g, " ").replace(/&amp;/g, "&").replace(/&nbsp;/g, " ").replace(/\s+/g, " ").trim();
}

function message(sender: string, recipients: string[], ago: number, bodyHtml: string, extra: Partial<DemoMessageSeed> = {}) {
  return { sender, recipients, ago, bodyHtml, bodyText: plain(bodyHtml), ...extra };
}

const receiptHtml = `
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="font-family:Helvetica,Arial,sans-serif;color:#1f2933">
  <tr><td style="padding:24px 0 8px;font-size:20px;font-weight:bold">Cloudline</td></tr>
  <tr><td style="padding:0 0 16px;font-size:14px;color:#52606d">Invoice CL-20931 · Paid with card ending 4242</td></tr>
  <tr><td>
    <table width="100%" cellpadding="8" cellspacing="0" style="border-collapse:collapse;font-size:14px">
      <tr style="background:#f5f7fa"><th align="left">Item</th><th align="right">Amount</th></tr>
      <tr><td style="border-bottom:1px solid #e4e7eb">Team plan · 12 seats</td><td align="right" style="border-bottom:1px solid #e4e7eb">$288.00</td></tr>
      <tr><td style="border-bottom:1px solid #e4e7eb">Managed database · 2 nodes</td><td align="right" style="border-bottom:1px solid #e4e7eb">$140.00</td></tr>
      <tr><td style="border-bottom:1px solid #e4e7eb">Bandwidth overage</td><td align="right" style="border-bottom:1px solid #e4e7eb">$12.40</td></tr>
      <tr><td><strong>Total</strong></td><td align="right"><strong>$440.40</strong></td></tr>
    </table>
  </td></tr>
  <tr><td style="padding:16px 0;font-size:12px;color:#7b8794">Questions about this invoice? Reply to this email and our billing team will help.</td></tr>
</table>`;

const metricsHtml = `
<div style="font-family:Georgia,serif;color:#243b53;max-width:560px">
  <h2 style="font-size:22px;margin:0 0 4px">Your week at a glance</h2>
  <p style="color:#627d98;margin:0 0 16px">Harborlight workspace · last 7 days</p>
  <table width="100%" cellpadding="10" cellspacing="0" style="border-collapse:collapse;font-family:Helvetica,Arial,sans-serif;font-size:14px">
    <tr><td style="background:#f0f4f8"><strong>Weekly active teams</strong></td><td align="right" style="background:#f0f4f8">1,284 <span style="color:#2f855a">▲ 6.2%</span></td></tr>
    <tr><td><strong>Median time to first reply</strong></td><td align="right">3h 12m <span style="color:#2f855a">▼ 18m</span></td></tr>
    <tr><td style="background:#f0f4f8"><strong>Trial → paid conversion</strong></td><td align="right" style="background:#f0f4f8">14.8% <span style="color:#c53030">▼ 0.4 pts</span></td></tr>
  </table>
  <p style="font-family:Helvetica,Arial,sans-serif;font-size:13px;color:#627d98">Tip: segment conversion by signup source to see where the dip came from.</p>
</div>`;

const flightHtml = `
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="font-family:Helvetica,Arial,sans-serif;color:#102a43">
  <tr><td style="padding:20px;background:#102a43;color:#ffffff;font-size:18px;font-weight:bold">You're all set, Maya</td></tr>
  <tr><td style="padding:16px 20px;font-size:14px">Confirmation code <strong style="letter-spacing:2px">K7QX2M</strong></td></tr>
  <tr><td style="padding:0 20px 20px">
    <table width="100%" cellpadding="6" cellspacing="0" style="font-size:14px;border:1px solid #d9e2ec">
      <tr><td><strong>SFO → LIS</strong></td><td>Mon, Oct 13</td><td>6:05 PM → 2:40 PM +1</td></tr>
      <tr><td><strong>LIS → SFO</strong></td><td>Fri, Oct 17</td><td>11:20 AM → 3:15 PM</td></tr>
    </table>
  </td></tr>
  <tr><td style="padding:0 20px 20px;font-size:12px;color:#627d98">Check-in opens 24 hours before departure.</td></tr>
</table>`;

const readerHtml = `
<div style="font-family:Georgia,serif;max-width:560px;color:#2d3748">
  <h1 style="font-size:26px;margin:0">The Weekend Reader</h1>
  <p style="color:#718096;margin:4px 0 20px">Issue 112 · Slow mornings, long reads</p>
  <h3 style="margin:0 0 6px">Why cities are planting tiny forests</h3>
  <p style="margin:0 0 16px">Dense, native plantings the size of a tennis court are cooling neighborhoods and bringing back birdsong.</p>
  <h3 style="margin:0 0 6px">The case for writing letters again</h3>
  <p style="margin:0 0 16px">A handwritten note takes ten minutes and is remembered for years.</p>
  <h3 style="margin:0 0 6px">One recipe: charred leeks with hazelnut butter</h3>
  <p style="margin:0">Twenty minutes, one pan, and the best thing you'll eat this week.</p>
</div>`;

/** A function rather than a constant so a normal build can drop this module entirely. */
function threadSeeds(): ThreadSeed[] {
  return [
    {
      id: "launch-plan",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Q4 launch plan — final review",
      labels: ["INBOX", "STARRED", "launch"],
      unread: true,
      starred: true,
      summary: "- Priya shared the final Q4 launch plan for sign-off by Thursday\n- Pricing page and onboarding changes are locked; press embargo lifts Oct 21\n- Open question: whether to stage the rollout by region",
      messages: [
        message(people.priya, [me.work, people.theo, people.jordan], 190, paragraphs(
          "Hi all — attached is the final draft of the Q4 launch plan. The big changes since last week:",
          "<strong>1.</strong> Pricing page copy is locked.<br><strong>2.</strong> Onboarding v3 ships behind a flag on day one.<br><strong>3.</strong> The press embargo lifts Tuesday, Oct 21 at 9am PT.",
          "Could everyone sign off by Thursday? Happy to walk through it live.",
          "— Priya",
        ), { attachments: [{ id: "launch-plan-pdf", filename: "Q4-launch-plan-v7.pdf", mimeType: "application/pdf", size: 2_482_113 }] }),
        message(me.work, [people.priya, people.theo, people.jordan], 150, paragraphs(
          "Looks great. One question: do we want to stage the rollout by region, or go global at once?",
          "Staging would give support a gentler first week.",
        )),
        message(people.priya, [me.work, people.theo, people.jordan], 24, paragraphs(
          "Good call, Maya. Let's stage it: North America on day one, then EU and APAC 48 hours later.",
          "I'll update the plan and put 30 minutes on the calendar for Thursday to confirm.",
        )),
      ],
    },
    {
      id: "design-crit",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Design crit: onboarding flow v3",
      labels: ["INBOX", "launch"],
      unread: true,
      messages: [
        message(people.theo, [me.work, people.elena], 52, paragraphs(
          "Hey Maya — the v3 onboarding prototype is ready for crit.",
          "The biggest change is that we ask for the team name <em>after</em> the first inbox connects, which cut drop-off by a third in hallway tests.",
          "Can you take a look before Wednesday's crit? Three things I'd love your eye on: the empty state, the progress indicator, and the invite step.",
        ), { attachments: [{ id: "onboarding-v3", filename: "onboarding-v3-flows.png", mimeType: "image/png", size: 845_220 }] }),
      ],
    },
    {
      id: "move-1on1",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Can we move our 1:1?",
      labels: ["INBOX"],
      unread: true,
      messages: [
        message(people.daniel, [me.work], 75, paragraphs(
          "Hi Maya, something came up Thursday afternoon. Could we move our 1:1 to Friday morning, maybe 10:30?",
          "Thanks!<br>Daniel",
        )),
      ],
    },
    {
      id: "renewal",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Contract renewal — Brightwater Co-op",
      labels: ["INBOX", "customers"],
      unread: false,
      starred: true,
      messages: [
        message(people.marcus, [me.work], 60 * 26, paragraphs(
          "Hi Maya,",
          "We're heading into budget season and I'd like to lock in our renewal. The team has grown from 18 to 31 people, so we'll need more seats.",
          "Could you send over updated pricing for 35 seats on an annual plan? A short call next week would also be great.",
          "Best,<br>Marcus Webb<br>Operations Director, Brightwater Co-op",
        )),
        message(me.work, [people.marcus], 60 * 24, paragraphs(
          "Thanks Marcus — great to hear the team is growing! I'll have pricing to you by Friday and will send a few times for a call.",
        )),
      ],
    },
    {
      id: "security-review",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Security review sign-off for launch",
      labels: ["INBOX", "launch"],
      messages: [
        message(me.work, [people.aisha], 60 * 30, paragraphs(
          "Hi Aisha — could you confirm the security review for the new sharing permissions is complete before we launch?",
        )),
        message(people.aisha, [me.work], 60 * 5, paragraphs(
          "Almost there. The pen test came back clean; I'm waiting on one last fix for link expiry. Expect sign-off by Wednesday.",
        )),
      ],
    },
    {
      id: "interview-notes",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Customer interview notes: 6 teams, 3 themes",
      labels: ["INBOX", "customers"],
      messages: [
        message(people.jordan, [me.work, people.priya], 60 * 7, `
          <p>Summary from this week's six customer interviews:</p>
          <ul>
            <li><strong>Shared inboxes</strong> — four of six teams want a lighter way to hand off threads.</li>
            <li><strong>Search</strong> — people trust search more when results show <em>why</em> they matched.</li>
            <li><strong>Mobile</strong> — mostly triage, rarely composing.</li>
          </ul>
          <p>Full notes and recordings are in the research folder. Happy to present at Friday's product review.</p>`),
      ],
    },
    {
      id: "offsite",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Offsite logistics — Lisbon, Oct 14–16",
      labels: ["INBOX", "travel"],
      messages: [
        message(people.sam, [me.work, people.priya, people.theo, people.daniel], 60 * 9, paragraphs(
          "Olá team! The offsite details are coming together.",
          "📍 We'll be at a converted warehouse in Marvila, 15 minutes from the city center.<br>🗓 Tuesday–Thursday, Oct 14–16<br>🍽 Team dinner Wednesday night",
          "The itinerary is attached. Please book flights by the end of the week and add them to the tracker.",
        ), { attachments: [{ id: "offsite-itinerary", filename: "lisbon-offsite-itinerary.pdf", mimeType: "application/pdf", size: 612_004 }] }),
      ],
    },
    {
      id: "hiring",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Re: Hiring loop for Senior Product Designer",
      labels: ["INBOX", "hiring"],
      messages: [
        message(people.elena, [me.work], 60 * 22, paragraphs(
          "We have two strong finalists for the Senior Product Designer role. Could you join the portfolio review Thursday at 2?",
        )),
        message(me.work, [people.elena], 60 * 21, paragraphs("Yes, count me in. Can you share the portfolios beforehand?")),
        message(people.elena, [me.work], 60 * 11, paragraphs("Done — both are in the hiring folder, with my notes alongside.")),
      ],
    },
    {
      id: "ci-pr",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "[harborlight/app] PR #482: Faster thread search",
      labels: ["INBOX", "notifications"],
      unread: true,
      messages: [
        message(people.builds, [me.work], 38, `
          <p><strong>Theo Alvarez</strong> requested your review on <strong>#482 Faster thread search</strong>.</p>
          <p style="font-family:Menlo,Consolas,monospace;font-size:13px;background:#f6f8fa;padding:8px">+412 −96 · 14 files changed · checks passing</p>
          <p>Search now uses an incremental index, cutting the p95 query time from 180 ms to 22 ms.</p>`,
        { unsubscribe: { methods: ["web"], listId: "app.harborlight.example" } }),
      ],
    },
    {
      id: "ci-deploy",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Deploy succeeded: production v4.18.0",
      labels: ["INBOX", "notifications"],
      messages: [
        message(people.builds, [me.work], 60 * 4, paragraphs("Production deploy <strong>v4.18.0</strong> finished in 6m 41s. All health checks are green."),
          { unsubscribe: { methods: ["web"], listId: "app.harborlight.example" } }),
      ],
    },
    {
      id: "metrics",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Your weekly metrics digest",
      labels: ["INBOX", "newsletters"],
      messages: [message(people.metrics, [me.work], 60 * 14, metricsHtml, { unsubscribe: { methods: ["oneClick"], listId: "digest.pulsemetrics.example" } })],
    },
    {
      id: "invoice",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Your Cloudline invoice for September",
      labels: ["INBOX", "receipts"],
      messages: [message(people.hosting, [me.work], 60 * 28, receiptHtml, {
        attachments: [{ id: "invoice-pdf", filename: "Cloudline-CL-20931.pdf", mimeType: "application/pdf", size: 88_310 }],
      })],
    },
    {
      id: "standup",
      accountId: SHOWCASE_WORK_ACCOUNT,
      subject: "Standup notes — Monday",
      labels: ["launch"],
      archived: true,
      messages: [message(people.daniel, [me.work, people.theo, people.jordan], 60 * 30, paragraphs("Yesterday: search indexing. Today: onboarding flag. Blockers: none."))],
    },
    {
      id: "dinner",
      accountId: SHOWCASE_PERSONAL_ACCOUNT,
      subject: "Dinner Saturday?",
      labels: ["INBOX"],
      unread: true,
      messages: [
        message(people.lena, [me.personal], 95, paragraphs(
          "Hey you! We're finally trying that new Basque place on Saturday — want to come? 7:30, and Ben's welcome too.",
          "Let me know and I'll change the reservation 🥂",
        )),
      ],
    },
    {
      id: "birthday",
      accountId: SHOWCASE_PERSONAL_ACCOUNT,
      subject: "Re: Mom's birthday plans",
      labels: ["INBOX", "STARRED", "family"],
      starred: true,
      messages: [
        message(people.ben, [me.personal], 60 * 36, paragraphs("I was thinking a picnic at the arboretum, then dinner at home. Thoughts?")),
        message(me.personal, [people.ben], 60 * 34, paragraphs("Love it. I'll bake the lemon cake and bring the good blanket.")),
        message(people.ben, [me.personal], 60 * 3, paragraphs("Perfect. I'll handle flowers and the playlist. Sunday the 12th, 1pm?")),
      ],
    },
    {
      id: "flight",
      accountId: SHOWCASE_PERSONAL_ACCOUNT,
      subject: "Your flight to Lisbon is confirmed",
      labels: ["INBOX", "travel"],
      messages: [message(people.airline, [me.personal], 60 * 19, flightHtml, {
        attachments: [{ id: "boarding", filename: "itinerary-K7QX2M.pdf", mimeType: "application/pdf", size: 154_882 }],
      })],
    },
    {
      id: "trail-photos",
      accountId: SHOWCASE_PERSONAL_ACCOUNT,
      subject: "Photos from the trail run",
      labels: ["INBOX", "family"],
      messages: [message(people.ben, [me.personal], 60 * 40, paragraphs("Still can't believe we made it to the summit before the fog rolled in. A few favorites attached!"), {
        attachments: [
          { id: "trail-1", filename: "summit.jpg", mimeType: "image/jpeg", size: 3_204_551 },
          { id: "trail-2", filename: "ridge-line.jpg", mimeType: "image/jpeg", size: 2_871_090 },
        ],
      })],
    },
    {
      id: "reader",
      accountId: SHOWCASE_PERSONAL_ACCOUNT,
      subject: "The Weekend Reader — Issue 112",
      labels: ["INBOX", "newsletters"],
      messages: [message(people.reader, [me.personal], 60 * 44, readerHtml, { unsubscribe: { methods: ["oneClick", "mailto"], listId: "weekendreader.example" } })],
    },
    {
      id: "library",
      accountId: SHOWCASE_PERSONAL_ACCOUNT,
      subject: "Your hold is ready for pickup",
      labels: ["INBOX"],
      messages: [message(people.library, [me.personal], 60 * 50, paragraphs("Good news! <strong>A Field Guide to Clouds</strong> is waiting for you at the Main Branch holds shelf until Friday."))],
    },
    {
      id: "prize",
      accountId: SHOWCASE_PERSONAL_ACCOUNT,
      subject: "You've been selected!!!",
      labels: ["TRASH"],
      trashed: true,
      messages: [message(people.prize, [me.personal], 60 * 60, paragraphs("Claim your reward today."))],
    },
  ];
}

function buildThreads(now: number): { threads: Thread[]; messages: DemoDataset["messages"] } {
  const minutesAgo = (ago: number) => new Date(now - ago * 60_000).toISOString();
  const threads: Thread[] = [];
  const messages: DemoDataset["messages"] = {};
  for (const seed of threadSeeds()) {
    const self = seed.accountId === SHOWCASE_WORK_ACCOUNT ? me.work : me.personal;
    const seeded = seed.messages.map(({ ago, ...rest }) => ({ ...rest, sentAt: minutesAgo(ago) }));
    const latest = seeded.at(-1)!;
    const latestInbound = [...seeded].reverse().find((candidate) => candidate.sender !== self) ?? latest;
    messages[seed.id] = seeded;
    threads.push({
      id: seed.id,
      providerThreadId: `showcase-${seed.id}`,
      subject: seed.subject,
      snippet: latest.bodyText.slice(0, 140),
      participants: [...new Set(seeded.map((candidate) => candidate.sender))].sort(),
      lastMessageAt: latest.sentAt,
      lastReceivedAt: latestInbound.sentAt,
      unread: seed.unread ?? false,
      starred: seed.starred ?? false,
      archived: seed.archived ?? false,
      trashed: seed.trashed ?? false,
      labels: seed.labels,
      accountId: seed.accountId,
      summary: seed.summary ?? null,
      summaryGeneratedAt: seed.summary ? minutesAgo(10) : null,
      summaryRevision: seed.summary ? latest.sentAt : null,
      hasAttachments: seeded.some((candidate) => (candidate.attachments?.length ?? 0) > 0),
    });
  }
  return { threads, messages };
}

function localDate(date: Date) {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** Events across the local Monday–Friday week containing `now`, plus next week's offsite. */
function buildSchedule(now: Date): ScheduleEvent[] {
  const monday = new Date(now);
  monday.setHours(0, 0, 0, 0);
  monday.setDate(monday.getDate() - ((monday.getDay() + 6) % 7));
  const slot = (day: number, time: string, minutes: number) => {
    const [hour, minute] = time.split(":").map(Number);
    const start = new Date(monday);
    start.setDate(monday.getDate() + day);
    start.setHours(hour, minute, 0, 0);
    return { start: start.toISOString(), end: new Date(start.getTime() + minutes * 60_000).toISOString() };
  };
  const day = (offset: number) => {
    const date = new Date(monday);
    date.setDate(monday.getDate() + offset);
    return localDate(date);
  };
  const work = SHOWCASE_WORK_ACCOUNT;
  const events: ScheduleEvent[] = [];
  for (let weekday = 0; weekday < 5; weekday += 1) {
    events.push({ id: `standup-${weekday}`, accountId: work, title: "Team standup", allDay: false, conferenceUrl: "https://meet.harborlight.example/standup", ...slot(weekday, "09:30", 15) });
  }
  events.push(
    { id: "roadmap", accountId: work, title: "Roadmap sync", allDay: false, location: "Room 4B", attendees: ["priya@harborlight.example", "theo@harborlight.example"], responseStatus: "accepted", canRespond: true, ...slot(0, "11:00", 60) },
    { id: "focus-1", accountId: work, title: "Focus time", allDay: false, ...slot(0, "14:00", 120) },
    { id: "crit", accountId: work, title: "Design crit: onboarding v3", allDay: false, location: "Studio", responseStatus: "needsAction", canRespond: true, ...slot(1, "10:30", 60) },
    { id: "lunch", accountId: SHOWCASE_PERSONAL_ACCOUNT, title: "Lunch with Lena", allDay: false, location: "Juniper Café", ...slot(1, "12:30", 60) },
    { id: "brightwater", accountId: work, title: "Brightwater renewal call", allDay: false, conferenceUrl: "https://meet.harborlight.example/brightwater", attendees: ["marcus@brightwater.example"], responseStatus: "tentative", canRespond: true, ...slot(1, "15:00", 30) },
    { id: "interviews", accountId: work, title: "Customer interviews readout", allDay: false, ...slot(2, "13:00", 45) },
    { id: "focus-2", accountId: work, title: "Focus time", allDay: false, ...slot(2, "15:00", 90) },
    { id: "launch-review", accountId: work, title: "Q4 launch plan sign-off", allDay: false, location: "Room 2A", ...slot(3, "10:00", 30) },
    { id: "portfolio", accountId: work, title: "Portfolio review: Senior Designer", allDay: false, ...slot(3, "14:00", 60) },
    { id: "one-on-one", accountId: work, title: "1:1 Daniel / Maya", allDay: false, attendees: ["daniel@harborlight.example"], ...slot(4, "10:30", 30) },
    { id: "product-review", accountId: work, title: "Product review", allDay: false, ...slot(4, "13:00", 60) },
    { id: "yoga", accountId: SHOWCASE_PERSONAL_ACCOUNT, title: "Yoga", allDay: false, location: "Riverside Studio", ...slot(4, "18:00", 60) },
    { id: "ship-week", accountId: work, title: "Launch week 🚀", allDay: true, start: day(3), end: day(5) },
    { id: "offsite", accountId: work, title: "Lisbon offsite", allDay: true, location: "Lisbon", start: day(8), end: day(11) },
  );
  return events;
}

function buildTasks(now: Date): ThreadTask[] {
  const stamp = new Date(now.getTime() - 2 * 3_600_000).toISOString();
  const inDays = (days: number) => {
    const date = new Date(now);
    date.setDate(date.getDate() + days);
    return localDate(date);
  };
  const today = (time: string) => {
    const [hour, minute] = time.split(":").map(Number);
    const date = new Date(now);
    date.setHours(hour, minute, 0, 0);
    return date.toISOString();
  };
  const base = { createdAt: stamp, updatedAt: stamp, completedAt: null, completionSource: null, sourceMessageId: null, notes: null, timeZone: null, repeatIntervalDays: null, evidenceText: null, waitAfter: null, goalId: null };
  return [
    { ...base, id: "task-launch", accountId: SHOWCASE_WORK_ACCOUNT, goalId: "goal-launch", threadId: "launch-plan", subjectSnapshot: "Q4 launch plan — final review", title: "Sign off on the Q4 launch plan", kind: "action", dueKind: "date", dueValue: inDays(2), status: "open", evidenceText: "Could everyone sign off by Thursday?" },
    { ...base, id: "task-pricing", accountId: SHOWCASE_WORK_ACCOUNT, goalId: "goal-renewals", threadId: "renewal", subjectSnapshot: "Contract renewal — Brightwater Co-op", title: "Send Marcus pricing for 35 seats", kind: "action", dueKind: "datetime", dueValue: today("16:00"), status: "open", notes: "Annual plan; include the volume discount tier." },
    { ...base, id: "task-renewal", accountId: SHOWCASE_WORK_ACCOUNT, goalId: "goal-renewals", threadId: "renewal", subjectSnapshot: "Contract renewal — Brightwater Co-op", title: "Follow up with Brightwater on renewal", kind: "follow_up", dueKind: "date", dueValue: inDays(7), repeatIntervalDays: 7, status: "open" },
    { ...base, id: "task-security", accountId: SHOWCASE_WORK_ACCOUNT, goalId: "goal-launch", threadId: "security-review", subjectSnapshot: "Security review sign-off for launch", title: "Waiting on Aisha's security sign-off", kind: "waiting_for", dueKind: "date", dueValue: inDays(1), status: "open" },
    { ...base, id: "task-crit", accountId: SHOWCASE_WORK_ACCOUNT, goalId: "goal-launch", threadId: "design-crit", subjectSnapshot: "Design crit: onboarding flow v3", title: "Review the onboarding v3 prototype", kind: "action", dueKind: "none", dueValue: null, status: "in_progress" },
    { ...base, id: "task-announcement", accountId: SHOWCASE_WORK_ACCOUNT, goalId: "goal-launch", threadId: null, subjectSnapshot: null, title: "Draft the launch announcement", kind: "action", dueKind: "none", dueValue: null, status: "completed", completionSource: "user", completedAt: stamp },
    { ...base, id: "task-flights", accountId: SHOWCASE_WORK_ACCOUNT, threadId: "offsite", subjectSnapshot: "Offsite logistics — Lisbon, Oct 14–16", title: "Book flights for the Lisbon offsite", kind: "action", dueKind: "none", dueValue: null, status: "completed", completionSource: "user", completedAt: stamp },
    { ...base, id: "task-agenda", accountId: SHOWCASE_WORK_ACCOUNT, threadId: null, subjectSnapshot: null, title: "Prepare the team retrospective agenda", kind: "action", dueKind: "date", dueValue: inDays(1), status: "open" },
    { ...base, id: "task-cake", accountId: SHOWCASE_PERSONAL_ACCOUNT, threadId: "birthday", subjectSnapshot: "Re: Mom's birthday plans", title: "Buy lemons and cake flour", kind: "action", dueKind: "date", dueValue: inDays(3), status: "open" },
  ];
}

/** Work goals for the current year, half, and quarter, so the Tasks workspace shows goal filters and progress. */
function buildGoals(now: Date): Goal[] {
  const createdAt = new Date(now.getTime() - 30 * 86_400_000).toISOString();
  const base = { accountId: SHOWCASE_WORK_ACCOUNT, notes: null, status: "active" as const, createdAt, updatedAt: createdAt, closedAt: null };
  const year = periodFor("year", now);
  const half = periodFor("half", now);
  const quarter = periodFor("quarter", now);
  return [
    { ...base, id: "goal-teams", title: "Become the default inbox for mid-size teams", horizon: "year", period: year, parentGoalId: null },
    { ...base, id: "goal-retention", title: "Grow revenue from existing customers", horizon: "half", period: half, parentGoalId: "goal-teams" },
    { ...base, id: "goal-launch", title: "Ship onboarding v3 with the Q4 launch", horizon: "quarter", period: quarter, parentGoalId: "goal-teams", notes: "North America first, then EU and APAC." },
    { ...base, id: "goal-renewals", title: "Renew every customer due this quarter", horizon: "quarter", period: quarter, parentGoalId: "goal-retention" },
  ];
}

export function buildShowcaseDataset(now: Date = new Date()): DemoDataset {
  const time = now.getTime();
  const daysAgo = (days: number) => new Date(time - days * 86_400_000).toISOString();
  const { threads, messages } = buildThreads(time);
  const connectedAt = daysAgo(120);
  return {
    accounts: [
      { email: SHOWCASE_WORK_ACCOUNT, displayName: "Maya Chen", color: "#4285F4", status: "connected", provider: "gmail", sortOrder: 0, connectedAt, lastSyncedAt: daysAgo(0.002) },
      { email: SHOWCASE_PERSONAL_ACCOUNT, displayName: "Maya Chen", color: "#34A853", status: "connected", provider: "gmail", sortOrder: 1, connectedAt, lastSyncedAt: daysAgo(0.002) },
    ],
    threads,
    messages,
    aiFixtures: {
      renewal: {
        summary: "- Brightwater is renewing its annual plan and needs pricing for 35 seats.\n- Marcus wants to finalize the renewal before budget season.",
        analysis: {
          hiddenCount: 0,
          proposals: [{
            type: "task", kind: "action", title: "Send Marcus pricing for 35 seats",
            notes: "Include annual pricing and the volume discount tier.",
            dueKind: "none", dueValue: null, timeZone: null, repeatIntervalDays: null,
            confidence: 0.96,
            evidence: { sourceMessageId: "renewal-message", excerpt: "Could you send over updated pricing for 35 seats on an annual plan?" },
          }],
        },
      },
      "launch-plan": {
        chat: {
          answer: "Onboarding v3 asks for the team name after the first inbox connects. The launch starts in North America, with EU and APAC following 48 hours later. Priya will update the plan before Thursday’s sign-off.",
          analysis: { proposals: [], hiddenCount: 0 }, replyDraft: null, availability: null,
          sources: threads.filter((thread) => thread.id === "design-crit").map(({ id, accountId, subject, lastMessageAt }) => ({ threadId: id, accountId, subject, lastMessageAt })),
          searched: threads.filter((thread) => thread.id === "design-crit").map(({ id, accountId, subject, lastMessageAt }) => ({ threadId: id, accountId, subject, lastMessageAt })),
        },
      },
    },
    details: {},
    labels: [
      { id: "INBOX", name: "Inbox", kind: "system", color: null },
      { id: "STARRED", name: "Starred", kind: "system", color: null },
      { id: "launch", name: "Launch", kind: "user", color: "#7b73ee" },
      { id: "customers", name: "Customers", kind: "user", color: "#0f9d8a" },
      { id: "hiring", name: "Hiring", kind: "user", color: "#e6892e" },
      { id: "travel", name: "Travel", kind: "user", color: "#2f80ed" },
      { id: "family", name: "Family", kind: "user", color: "#d9487c" },
      { id: "receipts", name: "Receipts", kind: "user", color: "#8a94a6" },
      { id: "newsletters", name: "Newsletters", kind: "user", color: "#b28b2c" },
      { id: "notifications", name: "Notifications", kind: "user", color: "#5c6bc0" },
    ],
    contacts: [
      { email: "priya@harborlight.example", displayName: "Priya Natarajan", sentCount: 48, receivedCount: 63, lastInteractedAt: daysAgo(0.02), pinned: true },
      { email: "theo@harborlight.example", displayName: "Theo Alvarez", sentCount: 31, receivedCount: 40, lastInteractedAt: daysAgo(0.04), pinned: false },
      { email: "daniel@harborlight.example", displayName: "Daniel Kim", sentCount: 27, receivedCount: 22, lastInteractedAt: daysAgo(0.05), pinned: false },
      { email: "marcus@brightwater.example", displayName: "Marcus Webb", sentCount: 12, receivedCount: 15, lastInteractedAt: daysAgo(1), pinned: true },
      { email: "aisha@harborlight.example", displayName: "Aisha Rahman", sentCount: 9, receivedCount: 11, lastInteractedAt: daysAgo(0.2), pinned: false },
      { email: "elena@harborlight.example", displayName: "Elena Rossi", sentCount: 14, receivedCount: 18, lastInteractedAt: daysAgo(0.5), pinned: false },
      { email: "jordan@harborlight.example", displayName: "Jordan Blake", sentCount: 8, receivedCount: 19, lastInteractedAt: daysAgo(0.3), pinned: false },
      { email: "sam@harborlight.example", displayName: "Sam Okafor", sentCount: 6, receivedCount: 10, lastInteractedAt: daysAgo(0.4), pinned: false },
      { email: "lena.ortiz@example.net", displayName: "Lena Ortiz", sentCount: 22, receivedCount: 25, lastInteractedAt: daysAgo(0.07), pinned: true },
      { email: "ben.chen@example.org", displayName: "Ben Chen", sentCount: 35, receivedCount: 38, lastInteractedAt: daysAgo(0.1), pinned: false },
    ],
    contactProfiles: [
      {
        id: "contact:priya@harborlight.example", displayName: "Priya Natarajan", role: "VP of Product Marketing", company: "Harborlight", location: "San Francisco, CA",
        bio: "Leads launches and positioning. Prefers written proposals before live reviews.", notes: "Loves a clear decision log. Out Oct 24–27.",
        links: ["https://harborlight.example/team/priya"], photoData: null, favorite: true, addresses: ["priya@harborlight.example", "priya.natarajan@harborlight.example"], sentCount: 48, receivedCount: 63, lastInteractedAt: daysAgo(0.02),
      },
      {
        id: "contact:marcus@brightwater.example", displayName: "Marcus Webb", role: "Operations Director", company: "Brightwater Co-op", location: "Portland, OR",
        bio: "Customer since 2023. Champion for the rollout across Brightwater's 31-person team.", notes: "Renewal due end of quarter; interested in the analytics add-on.",
        links: ["https://brightwater.example"], photoData: null, favorite: true, addresses: ["marcus@brightwater.example"], sentCount: 12, receivedCount: 15, lastInteractedAt: daysAgo(1),
      },
      {
        id: "contact:ben.chen@example.org", displayName: "Ben Chen", role: null, company: null, location: "Oakland, CA",
        bio: "Brother. Trail runner, terrible puns.", notes: "Mom's birthday: Sunday the 12th.",
        links: [], photoData: null, favorite: false, addresses: ["ben.chen@example.org"], sentCount: 35, receivedCount: 38, lastInteractedAt: daysAgo(0.1),
      },
    ],
    splitInboxes: [
      { id: "split-notifications", name: "Notifications", matchKind: "pattern", matchValue: "notifications", sortOrder: 0, createdAt: connectedAt, accountId: SHOWCASE_WORK_ACCOUNT },
      { id: "split-newsletters", name: "Newsletters", matchKind: "label", matchValue: "newsletters", sortOrder: 1, createdAt: connectedAt, accountId: SHOWCASE_WORK_ACCOUNT },
    ],
    tasks: buildTasks(now),
    goals: buildGoals(now),
    snippets: [
      { id: "snippet-schedule", name: "Share availability", body: "Happy to find a time! Here are a few windows that work on my side — pick whichever suits you best.", createdAt: connectedAt },
      { id: "snippet-review", name: "Will review", body: "Thanks for sending this over. I'll take a close look and get back to you by end of day tomorrow.", createdAt: connectedAt },
      { id: "snippet-intro", name: "Warm intro", body: "Looping in a colleague who'd be a great person to talk to about this — I'll let you two take it from here.", createdAt: connectedAt },
    ],
    calendarAccounts: [
      { email: SHOWCASE_WORK_ACCOUNT, connectedAt, status: "connected" },
      { email: SHOWCASE_PERSONAL_ACCOUNT, connectedAt, status: "connected" },
    ],
    calendarOptions: [
      { id: SHOWCASE_WORK_ACCOUNT, accountId: SHOWCASE_WORK_ACCOUNT, name: "Maya Chen", primary: true, selected: true, writable: true },
      { id: "launch@harborlight.example", accountId: SHOWCASE_WORK_ACCOUNT, name: "Launch", primary: false, selected: true, writable: true },
      { id: SHOWCASE_PERSONAL_ACCOUNT, accountId: SHOWCASE_PERSONAL_ACCOUNT, name: "Personal", primary: true, selected: true, writable: true },
    ],
    scheduleEvents: buildSchedule(now),
  };
}
