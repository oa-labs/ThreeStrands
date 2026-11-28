---
title: AI Assistance
nav: AI Assistance
summary: Optional summaries, suggestions, thread chat, reply drafts, draft review, meeting agendas, and Inbox Assistants, using your own provider and API key. Requests need your action unless you opt into Automatic Briefs or scheduled assistants.
reviewed: next
---

## How It Works {#how-it-works}

AI is off until you choose a provider. After that, ThreeStrands sends content only when you click a button marked with the sparkle icon (or run an assistant), unless you turn on **Automatic Briefs** or schedule an **Inbox Assistant**. It sends directly from your computer to the provider you choose, using your own API key. There is no ThreeStrands server in between.

## Set Up a Provider {#set-up-a-provider}

1. Open **Settings → AI Provider**.
2. Choose a **Provider**: OpenAI, Anthropic, OpenRouter, Fireworks, Straitly, or **Custom Endpoint** for any compatible service.
3. Paste your **API Key** and click **Save Key**. It's stored in your operating system's credential store.
4. Optionally choose a **Reasoning Model** under **Connection**. A **Custom Endpoint** needs a model name.
5. Click **Test Connection**.
6. Optionally open **Fast Model and Usage** to choose a cheaper **Fast Model** for quick tasks.

Once a provider is chosen, AI shows up in reading, writing, and contacts; **Meetings** starts off until you enable it. Under **Show AI In**, switch off or on **Reading**, **Writing**, **Contacts**, and **Meetings**. Under **Automatic Briefs**, choose whether to prepare briefs as you read: **Never** (the default), only for **People I've Emailed**, or for **All Conversations**.

ThreeStrands keeps one API key at a time, so enter a new key when you switch providers. Switching providers also clears the old provider's model names, so the new provider's default model is used until you choose another.

## Features {#features}

<ts-shot scene="thread-assist" alt="An AI brief of a conversation, with suggested tasks to review."></ts-shot>

### Reading {#reading}

- **Brief** — press <kbd>i</kbd> for a summary of what's new in the open conversation, with suggested tasks and meetings. A brief covers the messages since your last reply or your last brief, and reads earlier messages only for context, so it doesn't repeat what you already know; open **Earlier context** to expand that recap when shown. Open **Brief sources** to inspect the messages behind the brief's status and next steps (**Show message from …**), without repeating lines already in the brief. Each suggestion shows the evidence from the email, and nothing is added until you click **Add Task…** (use **⋯** to **Edit** or **Dismiss**). Press <kbd>Shift + a</kbd> for suggestions alone. Click the **Brief** heading to collapse it.
- **Calendar events** — **Get Brief** and **Ask About This Conversation** can find each event announced in an email at its stated date and time. Dated events without a time keep **All day** unchecked so you can enter the start and end time; review before adding.
- **Ask About This Conversation** — press <kbd>q</kbd> or <kbd>⌘/Ctrl + j</kbd> to ask about the open conversation. Questions only look beyond it when you tick **Search other mail** for that question. Choose **This account** or **All accounts**. Search uses the mail cached on this device. Open a cited passage to see the source message. **Use as Reply** turns an answer into a draft. Ask "When am I free Thursday afternoon?" and the answer shows open times from your calendar, already limited to the days and hours you asked for.
- **Automatic Briefs** — prepares the brief once you stay on a conversation for a few seconds. They skip mailing lists and conversations with no one else in them.

### Writing {#writing}

- **Draft Reply** — in the composer, press <kbd>⌘/Ctrl + j</kbd> to draft a reply from a short instruction and the sending account’s Writing Profile. Reply Assist marks the message you’re answering and previews the email context and profile before you generate.
- **Draft Review** — click **Review** below your message and feedback appears right away. Use **Apply Edit** on individual suggestions to preserve formatting, or preview and apply the full revision. If the feedback misses the point, add a goal for that email and click **Review Again**. **What was sent** under the feedback shows exactly what went to your provider. To avoid explaining yourself in every goal, fill in your profile for each account under **Settings → Writing Profile**: **About You** says who you are and what you do, and **Review Instructions** says how you want drafts reviewed. Draft Review shows the profile with the rest of what it sends, and suggests using a fact from it only as a suggestion you can see.
- **Find Times** — beside **Get Brief** at the top of the side panel, reads the conversation and your draft for when to meet, such as "next week" or "Thursday afternoon", and the app then finds open times on your calendar. Select times while reading and choose **Draft Reply With These Times**, or insert them into an open draft. Your calendar is never sent to the AI provider.

### Contacts {#contacts}

- **Enhance With AI** on a contact suggests empty profile details (including name, role, company, phone number, location, bio, and link) from your email for you to accept.

### Meetings {#meetings}

- Enable **Meetings** under **Settings → AI Provider → Show AI In** (off by default).
- In a meeting's **Preparation** panel, click **Prepare Agenda…** (or **Refresh Agenda…**) to review pre-selected local sources (**Invitation**, **Notes**, **Conversations**, **Open promises**, **Open commitments**, **Files**) within the character budget, then click **Generate Agenda** to create a source-cited **Suggested Agenda** you can insert into your meeting note via **Add Selected to Note** (with **Undo**).
- Any note checked **Exclude from AI** in **Note Details** is kept out of AI requests and disabled in source review.

Buttons marked with the sparkle icon, such as **Get Brief** and **Review**, are the ones that send something to your AI provider. Underneath a brief, a short note tells you whether it focused on the newest message(s) with earlier messages as background (or if new messages arrived since the brief).

Areas you switch off disappear from the app, along with their commands. Until you choose an AI provider, the **Brief** section beside a conversation is the one place that offers **Set Up AI**.

## Usage and Cost {#usage-and-cost}

The usage panel under **Settings → AI Provider → Fast Model and Usage** shows requests, tokens, and estimated cost for today and the last seven days. With OpenRouter or Straitly, model prices are filled in from the provider's model list; for other providers, add your model's prices to get accurate estimates. You can change a filled-in price.

**Activity**, below it, lists recent requests in columns: the conversation, the date, the provider and model, how many messages, and the tokens used. It's kept on this device and never includes email text.

You can cancel running AI work from its feature panel or **Request Controls**. Cancellation stops queued work or the app’s in-flight request; material already sent cannot be recalled and may still be billed. Up to two requests execute at once, with a bounded queue. Short rate-limit and temporary server failures retry at most twice; authentication failures and ambiguous network timeouts do not retry automatically.

**Request Provenance** in Request Controls lists recent attempts with source references and generation fingerprints. It keeps up to 30 days, 500 attempts, and 8 MB on this device, without mail text or provider replies. If the controls file cannot be read, **Recover AI Controls** preserves the unreadable file and lets you explicitly reset the controls and unknown reservations.

## What Is Shared {#what-is-shared}

| Feature | Trigger and context | Your control |
| --- | --- | --- |
| Brief and suggestions | On request; conversation messages, with earlier messages as context | Review proposed tasks and events before adding |
| Automatic Briefs | Opt-in after staying on a qualifying conversation | Choose Never, People I've Emailed, or All Conversations |
| Thread chat | Your question, conversation and chat context, open task context; other mail only with Search other mail | Choose attachment text to share; review replies and proposals |
| Draft Reply | Your instruction, reply context, and sending account Writing Profile | Review and send the draft yourself |
| Draft Review | Subject, authored text, goal, and account Writing Profile | What was sent shows the payload; apply revisions yourself |
| Find Times with AI | Conversation and draft to interpret the requested meeting range | Calendar availability is computed locally; calendar contents are not sent |
| Enhance With AI | Requested contact enrichment from correspondence | Accept suggested profile changes yourself |
| Meeting Preparation | On request (**Prepare Agenda…**); checked invitation, notes, conversations, promises, commitments, and file metadata | Review and uncheck sources before **Generate Agenda**; **Exclude from AI** blocks private notes |
| Inbox Assistants | Manual or scheduled (**Daily** / **Weekly**); candidate emails (plus related threads and calendar events for Sweeper); Sender Hygiene Advisor sends nothing | Enable, configure, pause, or run manually; review every suggestion before acting |

Draft Review excludes recipient fields, separately stored quoted history, and attachments. Text you put inside the authored draft itself is part of what it reviews. **Settings → AI Provider → Activity** records which provider, model, and message count each request used; a brief can be limited to recent messages rather than the entire history.

### Share an Attachment in Chat {#share-an-attachment-in-chat}

Type `@` in **Ask About This Conversation** and choose a readable attachment. Selecting it shares extracted text when you send the question and makes it available to the rest of that chat. Attachments are not automatically shared just because they belong to the email. Unsupported or unreadable PDF/text files fail safely without affecting the app (an unreadable PDF is skipped on later questions in the same chat); large files may be truncated, and the answer may miss later content. Review the listed sources and do not assume an answer covers a whole file.

### Provider Accounts and Billing {#provider-accounts-and-billing}

Requests go directly to the provider you choose. Its API billing, model access, and data-handling terms apply; a consumer chat subscription may not include API access. Check those terms in your provider account before sharing sensitive content. Usage estimates are not a bill. **Request Controls** offers an optional daily estimated spending limit for this device; enter model prices first. Requests reserve their full output allowance before sending, and uncertain or cancelled billing keeps that reservation. Your provider’s bill may differ. Use [AI troubleshooting](troubleshooting.html#ai-is-unavailable-or-failing) for connection failures.

## Inbox Assistants {#inbox-assistants}

Open **Agents** (<kbd>7</kbd>) to enable, configure, or pause **Stale Reads Summarizer**, **Expired & Already-Handled Sweeper**, **Aging Reply Drafter**, and **Sender Hygiene Advisor**. Assistants use the provider configured in **Settings → AI Provider** (completed-task checks in the sweeper and all of Sender Hygiene run locally without AI) and prepare proposals for your review without archiving or sending automatically. See [Inbox Assistants](inbox-assistants.html) for full controls and actions.
