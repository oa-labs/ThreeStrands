export const emailRenderingFixtures = {
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
  malformed: `
    <div style="position:fixed;top:0;left:0;z-index:99;animation:spin 1s;cursor:pointer">No overlay</div>
    <img src="javascript:alert(1)"><form action="https://example.com"><input value="bad"></form>
  `,
} as const;
