---
title: AI Assistance
nav: AI Assistance
summary: Optional summaries, suggestions, thread chat, reply drafts, and draft review, using your own provider and API key. Everything is off until you turn it on.
---

## How It Works

AI features are off by default. When you turn one on, ThreeStrands sends the content that feature needs directly from your computer to the provider you choose, using your own API key. There is no ThreeStrands server in between.

## Set Up a Provider

1. Open **Settings → AI Provider**.
2. Choose a **Provider**: OpenAI, Anthropic, OpenRouter, Fireworks, or **Custom Endpoint** for any compatible service.
3. Paste your **API Key** and click **Save Key**. It's stored in your operating system's credential store.
4. Click **Test Connection**.
5. Optionally choose a **Reasoning Model** and a cheaper **Fast Model**.
6. Turn on the **Features** you want.

ThreeStrands keeps one API key at a time, so enter a new key when you switch providers.

## Features

<ts-shot scene="thread-assist" alt="An AI brief of a conversation, with suggested tasks to review."></ts-shot>

- **Thread Summaries** — press <kbd>i</kbd> for a brief of the open conversation.
- **Suggestions** — press <kbd>Shift + a</kbd> to propose tasks from a conversation. Each one shows the evidence from the email, and nothing is added until you click **Review & Add Task**.
- **Proactive Suggestions** — prepares briefs as you read. You can limit them to people you've emailed.
- **Thread Chat** — press <kbd>q</kbd> to ask about the open conversation. Questions only look beyond it when you tick **Search all mail** for that question. **Use as Reply** turns an answer into a draft.
- **Draft Assist** — in the composer, press <kbd>⌘/Ctrl + j</kbd> for **Draft Reply With AI**, and use **Draft Review** for feedback against your goal. Suggestions are never sent automatically.
- **Contact Enrichment** — **Enhance with AI** on a contact suggests profile details from your email for you to accept.

Each feature shows what will be shared before anything is sent.

## Usage and Cost

The **Usage** panel in Settings shows requests, tokens, and estimated cost for today and the last seven days. Add your model's prices to get accurate estimates; OpenRouter reports exact costs itself.
