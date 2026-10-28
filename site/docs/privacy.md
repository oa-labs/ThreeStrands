---
title: Privacy and Local Storage
nav: Privacy and Local Storage
summary: Understand cached mail, remote images, credentials, and what leaves your computer.
reviewed: 0.179.0
---

## Keep Mail on This Device {#keep-mail-on-this-device}

Open **Settings → Privacy → Keep Mail on This Device For**. New installations default to **90 Days**; existing choices are preserved. Choose **30 Days**, **90 Days**, **1 Year**, or **Forever**.

Conversations whose newest message is older than the chosen period are removed from the local cache, including Trash. Starred conversations are kept unless they are in Trash. Mail stays on the server. This is local cleanup, not a server deletion rule.

Older messages may need an online [server search](search.html#local-results-and-older-mail) to appear again. Files lists attachments from locally synced mail; starring a file does not retain a separate offline copy. The mail-retention setting is not a promise to erase every kind of application data, exported file, or completed-send history.

## Remote Images {#remote-images}

Remote images are blocked by default. Choose **Load images** for a message, or enable **Settings → Privacy → Load Remote Images Automatically**. Loading images makes network requests to retrieve sender-hosted content through the app's native fetch path. Do not treat loading images or a familiar sender indicator as proof that mail is safe.

## Credentials and Local Data {#credentials-and-local-data}

Mail cache and search data live on this computer. Sign-in tokens, mail passwords, and AI API keys use the operating system's credential store. Local-first does not mean every local file is encrypted by ThreeStrands; use your operating system's device protection for local data.

Mail and calendar connections contact their providers. Optional [AI](ai.html#what-is-shared) sends selected context to your chosen AI provider. [Encrypted sync](sync.html) uses storage you choose, and [settings export](settings-and-sync.html) creates a password-encrypted file. Contact CSV and vCard exports are unencrypted.

## Sender and Attachment Checks {#sender-and-attachment-checks}

Checks run locally and explain suspicious addresses, links, and attachments. A known sender is not a promise that every message is safe. Read [Trust and Attachment Warnings](triage.html#trust-and-attachment-warnings) before confirming a new sending address.

## Diagnostics and Reports {#diagnostics-and-reports}

**Settings → Diagnostics** shows sync health and local crash-report controls. Review any error or report before sharing it publicly: account details and message identifiers may identify private activity. See [Get Help](get-help.html) and the [Privacy Policy](../privacy.html) for more detail.
