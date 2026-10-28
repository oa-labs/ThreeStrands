---
title: Connect Proton Mail Bridge
nav: Proton Mail Bridge
summary: Use the local Bridge servers and review their certificates before connecting.
reviewed: 0.179.0
---

## Before You Start {#before-you-start}

Run Proton Mail Bridge on the same computer as ThreeStrands and sign in there. Use Bridge's local IMAP and SMTP credentials, not your Proton account password. Automatic server discovery cannot find Bridge.

## Enter Bridge Settings {#enter-bridge-settings}

1. Open your account's mailbox details in Bridge.
2. In **Settings → Mail Accounts**, choose **Add Account → IMAP**.
3. Copy Bridge's host (usually `127.0.0.1`), IMAP and SMTP ports, security modes, username, and Bridge password.
4. Click **Check IMAP Certificate** and review the certificate against the local server you intended to connect to. Use **Trust This IMAP Certificate for This Server** only after checking it. Repeat the SMTP certificate check for the outgoing server.
5. Choose **Test and Save**, then verify the Sent, Archive, Drafts, Trash, and Junk mailbox mappings.

## If the Certificate Changes {#if-the-certificate-changes}

The account pauses and asks you to review the new certificate in **Settings → Mail Accounts**. Check that Bridge is running and that you are connecting to the intended local server before trusting a replacement. Messages waiting to send can go out once trust is restored; review **Outbox** first if they are no longer wanted.

## If Connection Fails {#if-connection-fails}

Keep Bridge running. Recheck its displayed credentials and security modes, including both server ports. A normal Proton password will not work here. Use [mail troubleshooting](troubleshooting.html#mail-stopped-arriving) for account and Diagnostics checks; do not disable certificate checks to bypass an unexplained change.
