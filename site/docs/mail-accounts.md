---
title: Manage Mail Accounts
nav: Mail Accounts
summary: Connect, recognize, reconnect, and remove accounts without confusing local setup with other devices.
reviewed: 0.179.0
---

## Connect an Account {#connect-an-account}

Use [Getting Started](getting-started.html#connect-gmail) for Gmail sign-in or [IMAP setup](getting-started.html#connect-an-imap-account) for password and app-password accounts. Proton Mail needs [Bridge](proton-bridge.html). Calendar connects separately under **Settings → Calendar Accounts**.

## Switch and Recognize Accounts {#switch-and-recognize-accounts}

In **Settings → Mail Accounts**, set an **Account Nickname**, **Sender Name**, and avatar color. Move accounts up or down to change their order. The sidebar selects one account; the combined view shows all accounts. <kbd>⌘/Ctrl + 1</kbd> through <kbd>⌘/Ctrl + 9</kbd> follow this order, and <kbd>⌘/Ctrl + 0</kbd> shows all accounts.

## Reconnect an Account {#reconnect-an-account}

If an account needs attention, open **Settings → Mail Accounts** and reconnect it. Complete Gmail sign-in in the system browser, or review IMAP credentials and certificate status. Check **Outbox** afterward: missed scheduled messages still wait for review, and an uncertain delivery should be checked in Sent before retrying.

## Accounts on Other Devices {#accounts-on-other-devices}

Accounts imported or learned through encrypted sync do not transfer credentials. Under **On Your Other Devices**, choose **Set Up Here** for the accounts you want on this computer and authenticate them. You do not have to connect every account on every computer.

## Remove an Account {#remove-an-account}

Review unsent mail in **Outbox** and unsynced work before removing an account. Local removal disconnects this installation; it does not delete the mailbox at your provider. **Remove on all devices** also removes it from your other synced devices and cancels waiting sends there. Those cancelled sends are marked failed rather than sending unexpectedly if the account returns.

Removal is not a substitute for deleting mail at the provider or erasing exported files. For problems receiving mail, [start with Diagnostics](troubleshooting.html#mail-stopped-arriving) instead of removing the account first.
