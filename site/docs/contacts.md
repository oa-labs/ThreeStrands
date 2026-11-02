---
title: Contacts
nav: Contacts
summary: Keep profiles, groups, and Keep in Touch reminders for the people you write to, and bring your address book in or out with CSV or vCard.
reviewed: 0.194.0
---

## The Address Book {#the-address-book}

Press <kbd>4</kbd> for Contacts. ThreeStrands builds a record of who you correspond with from your mail. Save someone as a contact to add details, and press <kbd>Tab</kbd> to switch between **All Contacts**, **Keep in Touch**, and **Groups**.

<ts-shot scene="contacts" alt="A contact profile with addresses, notes, correspondence history, and shared files."></ts-shot>

Click **New Contact** to add someone. A profile can hold several email addresses, a role and company, location, birthday, links, notes, and a photo. Click the heart to make someone a favorite; favorites appear first.

The conversation side panel's recent emails and shared files follow the message you are reading or the recipients you are writing to.

## Mac Contacts {#mac-contacts}

On macOS, open **Contacts → Manage Contacts… → Mac Contacts…**, choose **Allow Mac Contacts…**, grant access, then choose which address books to use. None are selected automatically. Sources may include iCloud, Google, and other accounts already configured in Apple Contacts; ThreeStrands does not ask for additional Google authorization.

Selected contacts contribute names, email addresses, companies, and job titles to Contacts and recipient suggestions. Phone numbers, photos, birthdays, and Apple lists are not connected in this version. **Never suggest this address** still controls autocomplete.

Apple Contacts stays read-only. Changes you save in ThreeStrands create independent local profiles; refreshing cannot overwrite their details, notes, groups, or reminders. Saved profiles follow your existing export and encrypted sync settings. Source-only contacts and the connection settings stay on this Mac.

Use **Refresh now** to update immediately; automatic refresh runs every two minutes while the app is running. **Disconnect Mac Contacts** removes downloaded entries and connection settings while keeping profiles you saved. Revoking permission in **System Settings → Privacy & Security → Contacts** clears downloaded entries on the next connection status check or automatic refresh.

## Groups {#groups}

Click **New Group** to create one, then **Add Members**. **Email Group** starts a new message to everyone in the group. Typing a group's name in To or Cc also offers the whole group, which expands into individual recipients.

When you choose **Add Members**, existing members and your pending selections
help prioritize people you often email together. Recommendations use cached
sent-mail history across accounts on this device, and never select or save a
member for you. Different devices may suggest different people; learned
relationships are excluded from encrypted sync.

### Recent Emails {#group-recent-emails}

Select a group to see **Recent Emails** in its side panel. It lists conversations with any current member from mail cached on this computer, including sent messages. The list follows your selected mail account; choose all accounts to see conversations across accounts. Changing group membership updates the history.

Click a conversation to open it and see who else took part, then return to the group and use **Add Members** for anyone you want to include. Expand the list to see more of the loaded conversations, or choose **Load older emails** when available. Contacts remembers your selected person separately in **All Contacts** and **Keep in Touch**, and your selected group in **Groups**, when you return from Mail.

## Keep in Touch {#keep-in-touch}

Set a **Keep in Touch** frequency on a profile, from weekly to yearly or a custom number of days. Email in either direction counts as being in touch. The Keep in Touch view groups people as Overdue, Due Today, This Week, and Later, and shows upcoming birthdays.

Use **Mark Contacted** when you've talked another way, or **Snooze** a reminder for a while. The Contacts icon in the sidebar shows how many people are due. Due reminders also appear in **Today** (<kbd>0</kbd>) beside your tasks and agenda; click a person to open their profile.

## Merge Duplicates {#merge-duplicates}

Click **Select**, choose two or more contacts, and choose **Merge…**. Pick the profile to keep; addresses, notes, links, favorites, and groups are combined.

## Import and Export {#import-and-export}

Open **Manage Contacts…**:

- **Import CSV or vCard…** shows a preview first. Records that share an email address with an existing contact are skipped.
- **Export CSV or vCard…** writes an unencrypted file of your saved profiles. Photos, groups, and Keep in Touch settings aren't included.
- **Manage recipient suggestions…** stops an address from being suggested when you write, on this computer only.
