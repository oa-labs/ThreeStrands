// Repeated boilerplate is current content, even when a conversation contains
// two reports with the same footer. Keep both table and flow layouts covered.
const tableReportFooter = `
  <table width="100%" cellpadding="0" cellspacing="0" style="background-color:#f8f9fa">
    <tr><td style="padding:30px 20px">
      <table width="100%" cellpadding="0" cellspacing="0"><tr>
        <td width="72"><img src="https://example.com/logo.png" width="64" alt="Example"></td>
        <td width="16" style="border-left:1px solid #5f6368"><img src="https://example.com/spacer.gif" width="16" alt=""></td>
        <td><table cellpadding="0" cellspacing="0"><tr><td style="font-size:10px;line-height:15px;color:#5f6368">
          <a style="color:#5f6368">© 2026 Example LLC<br>123 Example Street,<br>Sample City, ST 12345</a><br>
          <a href="https://example.com" style="color:#5f6368;text-decoration:underline">www.example.com</a>
        </td></tr></table></td>
      </tr></table>
    </td></tr>
    <tr><td style="padding:0 20px 30px;font-size:10px;line-height:15px;color:#5f6368">
      This email was sent because you requested monthly insights about your business profile.
      To stop receiving future reports, please unsubscribe <a href="https://example.com/unsubscribe">here</a>.
    </td></tr>
  </table>
  <img src="https://example.com/tracker.gif" width="1" height="1" alt="">
`;

const flowReportFooter = `
  <div style="background-color:#f4f4f4;padding:24px">
    <div style="display:flex;gap:16px;align-items:center">
      <img src="https://example.com/mark.png" width="48" alt="Example">
      <div style="border-left:1px solid #666;padding-left:16px;font-size:12px;line-height:18px">
        © 2026 Sample Organization<br>456 Demonstration Avenue<br>Demo Town, ST 67890
      </div>
    </div>
    <p style="font-size:12px;line-height:18px">
      You receive these account updates because you subscribed to our monthly activity reports.
      You can change your preferences or <a href="https://example.com/unsubscribe">unsubscribe from future updates</a> at any time.
    </p>
  </div>
`;

const quotedReportReply = `<div>On Monday, A. Sender wrote:</div><blockquote><p>Earlier quoted message.</p></blockquote>`;

// Every legacy dimension attribute spelling the sanitizer must read the way
// the browser does. Cells are empty so geometry comes from the attributes.
const legacyDimensionValues = [
  "20", "20px", "20PX", "20pt", "20em", "20 px", " 20", "&#10;20", "20.", "20.5px", "007",
  "20%", "20.%", "20 %", "20%px", "1e2", "0", "+20", "-20", ".5", "auto", "calc(20px)", "",
];
const legacyDimensionAttributes = "<p>Legacy dimension attributes</p>" + legacyDimensionValues.map((value) => `
  <div style="width:200px"><table width="${value}" cellspacing="${value}" cellpadding="${value}"><tr><td></td></tr></table></div>
  <table style="width:400px;table-layout:fixed" cellspacing="0" cellpadding="0"><tr>
    <td width="${value}" height="${value}"></td><td></td>
  </tr></table>`).join("");

export const emailRenderingFixtures = {
  legacyDimensionAttributes,
  zoomTable: `
    <table cellpadding="0" cellspacing="0" style="width:240px;font-family:Arial;font-size:12px;line-height:18px"><tr>
      <td style="padding:8px"><span>Oct 24, 2026</span></td>
      <td style="padding:8px"><a href="https://example.com/read">Read in Browser</a></td>
    </tr><tr><td colspan="2"><img src="https://example.com/icon.png" width="24" height="24" alt="Zoom icon"></td></tr></table>
    <p>On Monday, A. Sender wrote:</p><blockquote><p>Earlier quoted message.</p></blockquote>
  `,
  zoomFlow: `
    <style>.zoom-row { display:flex;gap:16px;width:240px;font-family:Arial;font-size:12px;line-height:18px;padding:8px; }</style>
    <div class="zoom-row"><span>Oct 24, 2026</span><a href="https://example.com/read">Read in Browser</a></div>
    <img src="https://example.com/icon.png" width="24" height="24" alt="Zoom icon">
    <p>On Monday, A. Sender wrote:</p><blockquote><p>Earlier quoted message.</p></blockquote>
  `,
  earlierTableReport: `<p>Your first report is ready.</p>${tableReportFooter}`,
  repeatedTableFooter: `<p>Your latest report includes new results.</p>${tableReportFooter}`,
  earlierFlowReport: `<p>Your previous account update.</p>${flowReportFooter}`,
  repeatedFlowFooter: `<p>New activity has been recorded.</p>${flowReportFooter}`,
  tableFooterWithEmptyQuote: `<p>Your latest report includes new results.</p>${tableReportFooter}<blockquote><div><br>&nbsp;</div></blockquote>`,
  flowFooterWithEmptyQuote: `<p>New activity has been recorded.</p>${flowReportFooter}<blockquote><span hidden>Hidden quoted message.</span></blockquote>`,
  tableFooterBeforeQuote: `<p>Your latest report includes new results.</p>${tableReportFooter}${quotedReportReply}`,
  flowFooterBeforeQuote: `<p>New activity has been recorded.</p>${flowReportFooter}${quotedReportReply}`,
  smallTextTable: `
    <table cellpadding="6" cellspacing="4"><tr><td style="font-size:10px;line-height:12px">
      Table copy <span style="font-size:80%">Relative footnote</span><span style="font-size:2.8em">Large relative heading</span>
      <font size="1">Legacy small text</font><a href="https://example.com" style="font:10px/12px Arial">Small link</a>
    </td></tr><tr><td style="font-size:0;line-height:0;height:8px">&nbsp;</td></tr></table>
  `,
  smallTextFlow: `
    <style>
      .small-copy { font-size:10px;line-height:120%; }
      .relative-copy { font-size:0.5em; }
      .rem-copy { font-size:0.5rem; }
      @media screen and (max-width:600px) { .small-copy { font-size:8px; } }
    </style>
    <article><h1 style="font-size:28px">Large heading</h1><p class="small-copy">Flow copy <em class="relative-copy">Nested small text</em></p><p class="rem-copy">Root relative text</p><div style="height:8px;font-size:1px">&nbsp;</div><p style="display:none;font-size:10px">Hidden copy</p></article>
  `,
  notification: `
    <div style="height:18px"></div>
    <table role="presentation" cellpadding="0" cellspacing="0" style="width:100%;table-layout:fixed">
      <tr><td style="vertical-align:top;width:44px;padding:0"><img width="32" height="32" src="https://example.com/avatar.png" style="border-radius:50%;display:block"></td>
      <td><h3 style="font:500 14px/20px Arial;margin:6px 0 2px">A. Sender</h3><div style="font:400 14px Arial;line-height:20px;margin-top:6px">A comment</div></td></tr>
    </table>
    <div style="margin:10px 0 0 42px;padding:2px"><a href="https://example.com/reply" style="display:inline-block;line-height:36px">Reply</a><a href="https://example.com/open" style="float:right;display:inline-block;line-height:36px">Open</a></div>
    <div style="height:20px"></div>
  `,
  transactional: `
    <style>
      .layout { display:flex; align-items:center; gap:12px; padding:24px; }
      @media screen and (max-width:600px) { .layout { display:block; padding:12px; } }
      @media (prefers-color-scheme: dark) { .dark-copy { color:#fff; background-color:#000; } }
    </style>
    <div class="layout"><strong class="dark-copy" style="font-size:18px;line-height:1.3">Invoice ready</strong><span>View details</span></div>
  `,
  newsletter: `
    <table role="presentation" style="width:100%;max-width:640px;margin:0 auto;border-collapse:separate;border-spacing:8px">
      <tr><td style="padding:16px;background-color:#f4f4f4"><h2>Monthly update</h2><p>Three concise highlights from the team.</p></td></tr>
    </table>
  `,
  // Column widths written as `width="20px"`: browsers read the px suffix, and
  // a fixed-layout table that loses them splits its width evenly instead.
  // Keep both a nested header and a flat icon column covered.
  pxGutterFixedHeader: `
    <table role="presentation" width="100%" cellspacing="0" cellpadding="0"><tr><td align="center">
      <table role="presentation" width="640" cellspacing="0" cellpadding="0" style="table-layout:fixed"><tr>
        <td width="20px">&nbsp;</td>
        <td align="center"><table role="presentation" width="600" cellspacing="0" cellpadding="0"><tr>
          <td align="left"><img src="https://example.com/logo.png" width="180" height="40" alt="logo"></td>
          <td width="100">&nbsp;</td>
          <td align="right" style="font:12px Arial"><a href="https://example.com/view">View online</a> | <a href="https://example.com/account">Account</a></td>
        </tr></table></td>
        <td width="20px">&nbsp;</td>
      </tr></table>
    </td></tr></table>
  `,
  pxIconColumnFixedTable: `
    <div style="padding:8px">
      <table role="presentation" cellspacing="0" cellpadding="0" style="width:100%;table-layout:fixed"><tr>
        <td width="56px" valign="top"><img src="https://example.com/icon.png" width="40px" height="40px" alt="status"></td>
        <td>Your weekly summary is ready. <a href="https://example.com/summary">Open summary</a></td>
      </tr></table>
    </div>
  `,
  // Column widths declared on <col>/<colgroup> rather than on cells. A
  // spreadsheet paste declares the table 0px wide and takes its real width
  // from the columns alone; losing them collapses every column and the
  // cells' overflow:hidden clips all content. Keep both a <col> list and a
  // bare <colgroup width> beside mixed <col> children covered.
  colWidthZeroWidthTable: `
    <p>Below are the options:</p>
    <table cellspacing="0" cellpadding="0" border="1" style="table-layout:fixed;width:0px;border-collapse:collapse">
      <colgroup><col width="158"><col width="601"><col width="150"></colgroup>
      <tbody>
        <tr style="height:21px"><td style="overflow:hidden;padding:2px 3px;border:1px solid rgb(204,204,204)">Option</td><td style="overflow:hidden;padding:2px 3px;border:1px solid rgb(204,204,204)">Details</td><td style="overflow:hidden;padding:2px 3px;border:1px solid rgb(204,204,204)">Price</td></tr>
        <tr style="height:21px"><td style="overflow:hidden;padding:2px 3px;border:1px solid rgb(204,204,204)">Standard</td><td style="overflow:hidden;padding:2px 3px;border:1px solid rgb(204,204,204)">Everything in the basic plan, delivered over a single afternoon session.</td><td style="overflow:hidden;padding:2px 3px;border:1px solid rgb(204,204,204)">300 USD</td></tr>
      </tbody>
    </table>
  `,
  colgroupWidthFixedTable: `
    <table role="presentation" cellspacing="0" cellpadding="0" style="table-layout:fixed;width:480px">
      <colgroup width="160px"></colgroup>
      <colgroup><col width="80"><col></colgroup>
      <tr><td style="overflow:hidden">Name</td><td style="overflow:hidden">Qty</td><td style="overflow:hidden">Notes</td></tr>
      <tr><td style="overflow:hidden">Widget</td><td style="overflow:hidden">2</td><td style="overflow:hidden">Ships next week</td></tr>
    </table>
  `,
  // A sender stylesheet resets spacing with !important, and individual
  // elements opt back in with their own inline !important. Inline important
  // declarations outrank stylesheet ones, so the opt-ins must keep priority.
  // Keep both a table button and a flow card covered.
  importantInlineTableButton: `
    <style>body, a, span { border: 0 !important; margin: 0 !important; padding: 0 !important; }</style>
    <table class="button" cellpadding="0" cellspacing="0"><tr>
      <td align="center" height="40" style="height:40px !important;background-color:#625afa;border-radius:6px">
        <a href="https://example.com/review" style="display:inline-block;height:40px !important;line-height:0 !important;padding:0 !important;color:#ffffff">
          <span style="display:block;padding:10px 16px !important;height:20px !important;line-height:20px !important;font-size:16px;white-space:nowrap">Review this sign-in</span>
        </a>
      </td>
    </tr></table>
  `,
  importantInlineFlowCard: `
    <style>div, p { margin: 0 !important; padding: 0 !important; }</style>
    <div class="card" style="padding:24px !important;background-color:#f4f4f4">
      <p style="margin:0 0 16px !important">Your export finished.</p>
      <p>Download it from your account.</p>
    </div>
  `,
  table: `
    <table cellpadding="12" cellspacing="4" width="100%"><thead><tr><th align="left">Item</th><th align="right">Amount</th></tr></thead><tbody><tr><td>Service</td><td align="right">$24</td></tr></tbody></table>
  `,
  flex: `
    <div style="display:flex;flex-direction:row;flex-wrap:wrap;align-items:center;justify-content:space-between;gap:8px"><span style="flex:1 1 180px">Flexible content</span><a href="https://example.com" style="flex:0 0 auto">Open</a></div>
  `,
  darkMode: `
    <style>@media (prefers-color-scheme: dark) { .panel { color:#fff; background-color:#202020; mix-blend-mode:normal; } }</style>
    <div class="panel" style="padding:12px">Theme-aware content</div>
  `,
  spacer: `<div></div><p>&nbsp;</p><table><tr><td></td></tr></table><p>Content after intentional spacing.</p>`,
  reply: `
    <p>Here is my answer.</p>
    <p>On Tue, Sep 15, 2026 at 7:28 AM A. Sender wrote:</p>
    <blockquote><p>Earlier message content.</p></blockquote>
  `,
  forwarded: `
    <div>FYI, see below.</div>
    <div>----- Forwarded message -----</div>
    <div>From: sender@example.com<br>Date: Tue, Sep 15, 2026<br>Subject: Details<br><br>Original details.</div>
  `,
  replyWrappedWroteLineBreak: `
    <p>Here is my answer.</p>
    <p>On Mon, Sep 21, 2026 at 2:33 PM A. Sender &lt;sender@example.com&gt;<br>wrote:</p>
    <blockquote><p>Earlier message content.</p></blockquote>
  `,
  replyWrappedWroteParagraphs: `
    <p>Here is my answer.</p>
    <p>On Mon, Sep 21, 2026 at 2:33 PM A. Sender &lt;sender@example.com&gt;</p>
    <p>wrote:</p>
    <blockquote><p>Earlier message content.</p></blockquote>
  `,
  // Quoted-history shapes. Each is structurally distinct; none relies on a
  // provider's class names or IDs, and every one is folded after the real
  // sanitize + linkify pass splits the attribution's address into a link.
  replyAttributionWithLinkedAddress: `
    <div dir="ltr">Thanks, that works.</div><br>
    <div><div dir="ltr">On Mon, Oct 5, 2026 at 9:00 AM A. Sender &lt;<a href="mailto:sender@example.com">sender@example.com</a>&gt; wrote:<br></div>
    <blockquote style="margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex"><div dir="ltr">Earlier message content.</div></blockquote></div>
  `,
  replyAttributionInsideCitation: `
    <div>Sounds good.</div>
    <div><br><blockquote type="cite"><div>On Oct 5, 2026, at 9:00 AM, A. Sender &lt;sender@example.com&gt; wrote:</div><br>
    <div><div>Earlier message content.</div></div></blockquote></div>
  `,
  replyRuleThenHeaderBlock: `
    <div><p>Thanks, will do.</p></div>
    <div></div><hr style="display:inline-block;width:98%">
    <div><font><b>From:</b> A. Sender &lt;sender@example.com&gt;<br><b>Sent:</b> Monday, October 5, 2026 9:00 AM<br><b>To:</b> Reader &lt;reader@example.com&gt;<br><b>Subject:</b> Re: Details</font><div>&nbsp;</div></div>
    <div><p>Earlier message content.</p><p>Second earlier paragraph.</p></div>
  `,
  replyCompleteHeaderBlockWithoutQuote: `
    <div><p>Thanks</p>
    <div><div style="border:none;border-top:solid #E1E1E1 1.0pt;padding:3.0pt 0in 0in 0in"><p><b>From:</b> A. Sender &lt;sender@example.com&gt;<br><b>Sent:</b> Monday, October 5, 2026 9:00 AM<br><b>To:</b> Reader<br><b>Subject:</b> Details</p></div></div>
    <p>Earlier message content.</p><p>Second earlier paragraph.</p></div>
  `,
  replyAngleQuotedLines: [
    "This is resolved now.<br><br>Reader<br><br>",
    "On 2026-10-05T14:32:48+00:00, A. Sender &lt;sender@example.com&gt; wrote:<br>",
    "&gt; Is this still happening?<br>&gt;<br>&gt; Earlier message content.<br>&gt;<br>&gt; Thanks",
  ].join(""),
  replyAngleQuotedLinesWithoutAttribution: [
    "<div>This is resolved now.</div>",
    "<div>&gt; Is this still happening?</div>",
    "<div>&gt;</div>",
    "<div>&gt; Earlier message content.</div>",
    "<div>&gt;</div>",
    "<div>&gt; Thanks</div>",
  ].join("\n"),
  inlineReplyBetweenQuotes: [
    "<div>Answers inline.</div>",
    "<div>&gt; First question?</div><div>&gt; More detail</div><div>&gt; More detail</div>",
    "<div>&gt; More detail</div><div>&gt; More detail</div>",
    "<div>My inline answer.</div>",
    "<div>&gt; Second question?</div>",
  ].join(""),
  // Repeated-text folding: the earlier message carries the same signature
  // the reply repeats above its quoted history.
  earlierMessageWithSignature: `
    <p>Can you check the feed?</p>
    <p>A. Sender<br>Engineering Lead, Example Co<br>555-0100</p>
  `,
  replyRepeatingSignature: `
    <p>Fixed now.</p>
    <p>A. Sender<br>Engineering Lead, Example Co<br>555-0100</p>
    <div>On Mon, Oct 5, 2026 at 9:00 AM, B. Reader &lt;reader@example.com&gt; wrote:</div>
    <blockquote><p>Earlier message content.</p></blockquote>
  `,
  // Small declared-size images beside text that must wrap. Table auto layout
  // must keep the icon at its declared size instead of compressing it.
  iconCalloutFlexTable: `
    <table role="presentation" width="100%" cellpadding="0" cellspacing="0"><tr>
      <td style="border-radius:8px;background-color:#f0f2f2;padding:12px 8px;gap:8px;align-items:flex-start;display:flex">
        <table role="presentation"><tr>
          <td style="padding-right:8px"><img src="https://example.com/info.png" alt="info" width="18" height="18"></td>
          <td style="font-size:15px;line-height:20px">A courier may contact you on the day of arrival. For everyone's safety, keep the entrance clear and well lit before the visit.</td>
        </tr></table>
      </td>
    </tr></table>
  `,
  iconCalloutTable: `
    <table width="100%" cellpadding="0" cellspacing="0" style="background-color:#eef3ff"><tr>
      <td valign="top" style="padding:12px"><img src="https://example.com/note.png" alt="note" width="24" height="24"></td>
      <td style="padding:12px 12px 12px 0;font-size:14px;line-height:20px">Your scheduled maintenance window starts tonight. Services may be briefly unavailable while the update completes, and no action is required.</td>
    </tr></table>
  `,
  // A height-only image relies on its natural aspect ratio for width, including
  // when a high-density source is larger than the intended display size.
  heightOnlyIconTable: `
    <table width="100%" cellpadding="0" cellspacing="0"><tr>
      <td valign="top"><img height="20" src="https://example.com/height-only.png" alt="mail"></td>
      <td width="13"></td>
      <td style="font-size:16px">This is a copy of an account notification. Your recovery address received this message so you can review the activity and update your account if necessary.</td>
    </tr></table>
  `,
  heightOnlyIconFlex: `
    <div style="display:flex;align-items:flex-start;gap:12px">
      <img height="20" src="https://example.com/height-only-wide.png" alt="notice" style="display:block">
      <p style="margin:0;font-size:18px">A scheduled service update is coming soon. Please review the details before the maintenance window begins and contact support with any questions.</p>
    </div>
    <style>.limited-icon { max-width:12px; }</style>
    <img height="20" src="https://example.com/height-only.png" alt="inline cap" style="max-width:10px">
    <img height="20" src="https://example.com/height-only.png" alt="stylesheet cap" class="limited-icon">
  `,
  // Images wider than a narrow reading pane must still scale to fit.
  heroInTable: `
    <table width="100%" cellpadding="0" cellspacing="0"><tr>
      <td style="padding:0 16px"><img src="https://example.com/hero.png" alt="hero" width="600" style="display:block"></td>
    </tr></table>
  `,
  heroInFlow: `<div style="padding:0 16px"><img src="https://example.com/banner.png" alt="banner" width="600"></div>`,
  // A whole message sent as preformatted text: long lines must wrap at the
  // pane edge rather than run past it.
  preformattedInFixedTable: `
    <table width="100%" cellpadding="0" cellspacing="0" style="table-layout:fixed;max-width:600px"><tr>
      <td valign="top"><pre><div style="font-family:inherit">Hello,

The scheduled maintenance for every workspace in your organization begins on Saturday morning and should finish within two hours of starting.
Please save any open work before then, because sessions that remain active during the maintenance window will be signed out automatically.

Thanks,
-Operations</div></pre></td>
    </tr></table>
  `,
  preformattedInFlow: `<pre style="font-family:monospace">Report summary: every scheduled export completed successfully overnight, and the next run is queued for the same time tomorrow evening.
Second line: no further action is required from you unless an export fails, in which case a separate alert will follow.</pre>`,
  // Author CSS that asks for unwrapped preformatted text still wins.
  preformattedAuthorNoWrap: `<style>pre.listing { white-space: pre; }</style><pre class="listing">column_one    column_two    column_three    column_four    column_five    column_six    column_seven</pre>`,
  // Background images placed and scaled inside their own box. A @2x sprite
  // without its size/position/repeat paints at natural size from the corner
  // and is cropped by the element.
  backgroundIconLinks: `
    <p style="text-align:center;font-size:14px">Follow along for new sessions.</p>
    <table width="100%" cellpadding="0" cellspacing="0"><tr>
      <td align="center" style="padding:16px 0;font-size:0px;line-height:0">
        <a href="https://example.com/video" style="display:inline-block;width:48px;height:48px;margin:0 12px;background:url('https://example.com/video_2x.png') center center/48px 48px no-repeat">&nbsp;</a>
        <a href="https://example.com/chat" style="display:inline-block;width:48px;height:48px;margin:0 12px;background:url('https://example.com/chat_2x.png') center center/48px 48px no-repeat">&nbsp;</a>
      </td>
    </tr></table>
  `,
  backgroundPanelLonghands: `
    <div style="width:320px;padding:24px;min-height:120px;background-color:#eef3ff;background-image:url(https://example.com/panel.png);background-size:cover;background-position:right 8px bottom 4px;background-repeat:no-repeat;background-origin:content-box;background-clip:padding-box">
      <p style="margin:0;font-size:16px">Your workspace is ready.</p>
    </div>
  `,
  // A max-width column centered with longhand auto margins. Keep both the
  // hybrid flow layout (an Outlook-only table hidden in a conditional
  // comment, a div column for everyone else) and a stylesheet-centered table
  // with no align attribute covered.
  centeredHybridColumn: `
    <div style="background:#ffffeb">
      <!--[if true]><table role="none" align="left" style="width:100%"><tr><td></td><td style="width:600px"><![endif]-->
      <!--[if !true]><! --><div class="centered" style="max-width:600px;margin-left:auto;margin-right:auto"><!-- <![endif]-->
        <div style="padding:40px 30px 0;overflow:auto"><p style="margin:0">Dark mode is now available on desktop.</p></div>
      <!--[if !true]><! --></div><!-- <![endif]-->
      <!--[if true]></td><td></td></tr></table><![endif]-->
    </div>
  `,
  centeredStyledTable: `
    <style>
      .centered { width:480px; margin-left:auto !important; margin-right:auto !important; }
    </style>
    <table role="presentation" class="centered" cellpadding="0" cellspacing="0">
      <tr><td style="padding:24px;background-color:#f4f4f4">Your order has shipped.</td></tr>
    </table>
  `,
  // Centering that comes only from a <center> wrapper. Keep both a table
  // with no align attribute (inside <section>, as templates nest it) and
  // inline-block flow content covered.
  centerWrappedTable: `
    <section>
      <center>
        <table role="presentation" class="centered" width="480" cellpadding="0" cellspacing="0">
          <tr><td style="padding:24px;background-color:#f4f4f4">About the program</td></tr>
        </table>
      </center>
    </section>
  `,
  centerWrappedInline: `
    <center><div class="centered" style="display:inline-block;width:320px;text-align:left"><img src="https://example.com/info.png" width="20" height="20" alt=""> Program FAQ</div></center>
  `,
  // Typography carried only by <font>. Keep a styled <font> in a header-cell
  // column layout (whose cells default to bold) and the legacy face/color/size
  // attributes in paragraph flow covered.
  fontStyledHeaderColumns: `
    <table role="presentation" cellpadding="0" cellspacing="0"><tr>
      <th align="left" valign="top" width="100"><font style="font-family:Arial, sans-serif;font-size:72px;font-weight:normal;line-height:50px;color:#946e3a">2X</font></th>
      <th align="left" width="450"><font style="font-size:15px;font-weight:normal;line-height:21px;color:#141414"><strong>Rewards</strong> on everyday purchases.<sup style="font-size:10px!important;line-height:0px!important">1</sup></font></th>
    </tr></table>
  `,
  fontAttributesInFlow: `
    <p><font face="Georgia, serif" color="#336699" size="2">Program details</font> and <font size="+2" color="red">key dates</font>.</p>
  `,
  malformed: `
    <div style="position:fixed;top:0;left:0;z-index:99;animation:spin 1s;cursor:pointer">No overlay</div>
    <img src="javascript:alert(1)"><form action="https://example.com"><input value="bad"></form>
  `,
} as const;
