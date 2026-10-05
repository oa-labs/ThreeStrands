export const emailRenderingFixtures = {
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
  malformed: `
    <div style="position:fixed;top:0;left:0;z-index:99;animation:spin 1s;cursor:pointer">No overlay</div>
    <img src="javascript:alert(1)"><form action="https://example.com"><input value="bad"></form>
  `,
} as const;
