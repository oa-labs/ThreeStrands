---
title: Connect Proton Mail Bridge
nav: Proton Mail Bridge
summary: Use the local Bridge servers and review their certificates before connecting.
reviewed: next
---

## Before You Start {#before-you-start}

Run Proton Mail Bridge on the same computer as ThreeStrands and sign in there. Use Bridge's local IMAP and SMTP credentials, not your Proton account password. Automatic server discovery cannot find Bridge.

## Enter Bridge Settings {#enter-bridge-settings}

1. Open your account's mailbox details in Bridge.
2. In **Settings → Mail Accounts**, choose **Add Account → IMAP**.
3. Choose Bridge's security modes first so custom local ports stay intact, then copy the host (usually `127.0.0.1`), IMAP (`1143`) and SMTP (`1025`) ports, username, and Bridge password.
4. Click **Check IMAP Certificate** and review the certificate against the local server you intended to connect to. Use **Trust This IMAP Certificate for This Server** only after checking it. Repeat the SMTP certificate check for the outgoing server.
5. Choose **Test and Save**, then under **Map Your Mailboxes**, verify **Sent**, **Archive**, **Drafts**, **Trash**, **Junk / Spam**, **All Mail (Aggregate)**, and **Label Storage** (or **Label Container Mailbox**). To configure extra aliases, see [IMAP Sending Addresses](mail-accounts.html#imap-sending-addresses).

## If the Certificate Changes {#if-the-certificate-changes}

The account pauses and asks you to review the new certificate in **Settings → Mail Accounts** (open **Server** to inspect and re-trust it; leave the password field blank to keep the saved Bridge password). Check that Bridge is running and that you are connecting to the intended local server before trusting a replacement. Messages waiting to send can go out once trust is restored; review **Outbox** first if they are no longer wanted.

## If Connection Fails {#if-connection-fails}

Keep Bridge running. Recheck its displayed credentials and security modes, including both server ports. A normal Proton password will not work here. Use [mail troubleshooting](troubleshooting.html#mail-stopped-arriving) for account and Diagnostics checks; do not disable certificate checks to bypass an unexplained change.
