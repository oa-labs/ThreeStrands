---
title: Search Your Mail
nav: Search
summary: Find messages locally and on the server with examples, operators, and search suggestions.
reviewed: 0.179.0
---

## Start a Search {#start-a-search}

Press <kbd>/</kbd> to search. Search looks through all your mail, including archived conversations, and shows results from this computer right away. When you stop typing, it also asks the server for older mail. Results still in your inbox are marked **Inbox**. Click **Inbox Only** to search just your inbox. Trash and Spam are left out unless you add `in:trash`, `in:spam`, or `in:anywhere`.

You can narrow a search with `from:`, `to:`, `subject:`, `before:` and `after:` (YYYY/MM/DD), `is:unread`, `is:starred`, `label:`, `in:`, and `has:attachment`. `to:` also finds mail where the person was on Cc or Bcc. Put a minus in front of a term to leave it out (`invoice -from:billing@example.com`), and use `OR` or braces for either-or (`from:ana OR from:bo`, or `{from:ana from:bo}`). As you type, suggestions appear below the search box: operator names, people from your contacts after `from:` or `to:` (choosing someone with more than one address searches all of them), your labels after `label:` or `in:`, and dates like **Past month** after `after:` or `before:`. Use the arrow keys and <kbd>Enter</kbd>, or click, to choose one. <kbd>Escape</kbd> closes the suggestions, and a second <kbd>Escape</kbd> clears the search. Click the empty search box to see your recent searches, which stay on this computer.


## Search Examples {#search-examples}

| Find | Search |
| --- | --- |
| Invoices from one person | `invoice from:ana@example.com` |
| Mail you sent to someone, including Cc or Bcc | `to:bo@example.com` |
| Messages with files | `has:attachment` |
| Mail after a date | `after:2026/10/01` |
| Either of two senders | `{from:ana@example.com from:bo@example.com}` |
| Exclude a sender | `invoice -from:billing@example.com` |
| Look in Trash | `in:trash` |
| Include Trash and Spam | `in:anywhere` |

## Operator Reference {#operator-reference}

| Operator | Meaning |
| --- | --- |
| `from:` / `to:` | Sender or recipient; `to:` includes Cc and Bcc |
| `subject:` | Words in the subject |
| `before:` / `after:` | Date in YYYY/MM/DD form |
| `is:unread` / `is:starred` | Unread or starred mail |
| `label:` / `in:` | Label or mailbox |
| `has:attachment` | Mail with an attachment |
| `-term` | Leave out a term or operator |
| `OR` / `{ ... }` | Either alternative |

## Local Results and Older Mail {#local-results-and-older-mail}

Local results are available offline for cached mail. Server results need a connection and valid account sign-in; a local search is not proof that older mail is absent from the server. [Retention](privacy.html#keep-mail-on-this-device) limits the cache, not the mail kept by your provider.

Search follows the current workspace: in **Files** it searches filenames and senders, and in **Contacts** it searches contacts. Return to **Mail** to use mail operators. If you still cannot find something, use [missing-message troubleshooting](troubleshooting.html#a-message-is-missing).
