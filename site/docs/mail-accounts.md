---
title: Manage Mail Accounts
nav: Mail Accounts
summary: Connect, recognize, reconnect, and remove accounts without confusing local setup with other devices.
reviewed: 0.192.2
---

## Connect an Account {#connect-an-account}

Use [Getting Started](getting-started.html#connect-gmail) for Gmail sign-in or [IMAP setup](getting-started.html#connect-an-imap-account) for password and app-password accounts. Proton Mail needs [Bridge](proton-bridge.html). Calendar connects separately under **Settings → Calendar Accounts**.

For IMAP servers that support label-based archiving, archived mail stays in All Mail and keeps its other labels. Setup does not require a separate Archive mailbox for those accounts. Other IMAP accounts continue to use their chosen Archive mailbox.

For IMAP servers with native label support, **Manage Labels** lets you create labels and apply or remove them without configuring a label folder container. Enter a full nested path to create a nested label. Rename or delete those labels in another mail client.

## Gmail Sending Addresses {#gmail-sending-addresses}

Add and verify aliases in Gmail's **Settings → Accounts → Send mail as**. ThreeStrands discovers them when it syncs; after changing them in Gmail, use **Settings → Mail Accounts → Sending Addresses → Refresh from Gmail** to update immediately. If your account only has its primary address, open **Aliases…** to find the instructions and refresh control. Accounts with aliases show the full list automatically, including the default address and any Reply-To destination. A custom Reply-To on the primary address stays visible even when the alias controls are collapsed. ThreeStrands reads these settings using your existing Gmail connection; it does not create or verify aliases for you.

Choose an address in the composer's **From** menu. New messages use Gmail's default address; replies choose the matching address in the original message's recipients or delivery headers. Each address can have its own defaults in **Settings → Signatures**. Primary-account sender names still use your ThreeStrands settings; alias names and Reply-To destinations come from Gmail.

Previously discovered addresses work offline, and drafts preserve the selected address across restarts and server-draft sync. An address removed from Gmail stays visible as unavailable in an existing draft so you can choose another. Before delivery, ThreeStrands checks that an alias is still verified; an unreachable Gmail connection keeps the message queued, and a removed alias stops delivery so you can return it to Drafts and choose a current address. Scheduled messages also wait if the alias cannot be verified.

Aliases are refreshed independently on each computer. The verified sending-address cache is excluded from settings exports and encrypted sync. Gmail plus-addresses appear only if they are explicitly configured as verified Send mail as addresses.

## Switch and Recognize Accounts {#switch-and-recognize-accounts}

In **Settings → Mail Accounts**, set an **Account Nickname**, **Sender Name**, and avatar color. Move accounts up or down to change their order. Choosing an account in the sidebar opens its mail, from any view; the combined view shows all accounts. <kbd>⌘/Ctrl + 1</kbd> through <kbd>⌘/Ctrl + 9</kbd> follow this order, and <kbd>⌘/Ctrl + 0</kbd> shows all accounts.

## Reconnect an Account {#reconnect-an-account}

If an account needs attention, open **Settings → Mail Accounts** and reconnect it. Complete Gmail sign-in in the system browser, or review IMAP credentials and certificate status. Check **Outbox** afterward: missed scheduled messages still wait for review, and an uncertain delivery should be checked in Sent before retrying.

## Accounts on Other Devices {#accounts-on-other-devices}

Accounts imported or learned through encrypted sync do not transfer credentials. Under **On Your Other Devices**, choose **Set Up Here** for the accounts you want on this computer and authenticate them. You do not have to connect every account on every computer.

## Remove an Account {#remove-an-account}

Review unsent mail in **Outbox** and unsynced work before removing an account. Local removal disconnects this installation; it does not delete the mailbox at your provider. **Remove on all devices** also removes it from your other synced devices and cancels waiting sends there. Those cancelled sends are marked failed rather than sending unexpectedly if the account returns.

Removal is not a substitute for deleting mail at the provider or erasing exported files. For problems receiving mail, [start with Diagnostics](troubleshooting.html#mail-stopped-arriving) instead of removing the account first.
