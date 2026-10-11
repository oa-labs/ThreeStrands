#!/bin/sh
# Seed deterministic test mail for the IMAP integration tests. Idempotent: a
# marker file guards re-seeding so container restarts keep stable UIDs.
#
# Messages are delivered with dovecot-lda so they land as real Maildir files
# with Dovecot-assigned UIDs. The set is intentionally small and fixed:
#   - 2 INBOX messages (one plain, one with a Message-ID we also drop into Sent
#     to exercise the "one message, two locations" identity rule);
#   - 1 Sent message (the shared Message-ID);
#   - 1 Archive message;
#   - 1 Trash message and 1 Junk message (index-synced, never reported);
#   - 1 labelled+foldered message (Slice 5b-2 run B): the same Message-ID in
#     INBOX, the user folder Folders/Projects and the label folder
#     Labels/Clients (one message, three locations);
#   - 1 old message ONLY in Folders/Projects (index-synced, never reported).
# Keep this in sync with the fixtures the Rust tests assert against.
set -eu

USER=test@threestrands.test
MARKER=/srv/mail/${USER}/.seeded
MAILROOT=/srv/mail/${USER}

if [ -f "$MARKER" ]; then
  echo "[seed] already seeded; skipping"
  exit 0
fi

deliver() {  # deliver <mailbox> <message-on-stdin>
  local box="$1"
  # doveadm save writes a message into the user's mailbox with a real
  # Dovecot-assigned UID (dovecot-lda is not shipped in this image).
  doveadm save -u "$USER" -m "$box" || return 1
}

echo "[seed] delivering test messages for $USER"

deliver "INBOX" <<'EOF'
From: alice@example.test
To: test@threestrands.test
Subject: Welcome to the test inbox
Message-ID: <inbox-welcome-0001@example.test>
Date: Mon, 06 Oct 2025 09:00:00 +0000

Plain INBOX message used by the read-only sync tests.
EOF

# This Message-ID is delivered to BOTH INBOX and Sent, so the provider must
# resolve it to one message with two imap_locations rows.
SHARED='<shared-bcc-self-0002@threestrands.test>'
deliver "INBOX" <<EOF
From: test@threestrands.test
To: bob@example.test
Subject: Bcc to self appears in two mailboxes
Message-ID: ${SHARED}
Date: Mon, 06 Oct 2025 10:00:00 +0000

A Bcc-to-self lands in INBOX and Sent; it is one message, two locations.
EOF
deliver "Sent" <<EOF
From: test@threestrands.test
To: bob@example.test
Subject: Bcc to self appears in two mailboxes
Message-ID: ${SHARED}
Date: Mon, 06 Oct 2025 10:00:00 +0000

A Bcc-to-self lands in INBOX and Sent; it is one message, two locations.
EOF

deliver "Archive" <<'EOF'
From: carol@example.test
To: test@threestrands.test
Subject: An archived message
Message-ID: <archive-old-0003@example.test>
Date: Sun, 01 Jun 2025 12:00:00 +0000

An older message in Archive, inside the per-folder header window.
EOF

# A Trash and a Junk message: run 3 syncs these as index-only locations
# (TRASH / SPAM labels) but never reports them as changed_threads — the live
# test asserts they are present locally yet not in any changed set.
deliver "Trash" <<'EOF'
From: dave@example.test
To: test@threestrands.test
Subject: A trashed message
Message-ID: <trash-0004@example.test>
Date: Mon, 06 Oct 2025 11:00:00 +0000

A message in Trash, index-synced but not reported.
EOF

deliver "Junk" <<'EOF'
From: spammer@example.test
To: test@threestrands.test
Subject: A junk message
Message-ID: <junk-0005@example.test>
Date: Mon, 06 Oct 2025 11:30:00 +0000

A message in Junk, index-synced but not reported.
EOF

# Slice 5b-2 run B: a message that is in INBOX AND copied into both a user
# folder (Folders/Projects -> folder:Folders/Projects) and a label-container
# child (Labels/Clients -> lf:Clients). Same Message-ID in all three places =
# ONE message with three locations, so the live test asserts the INBOX message
# carries INBOX + lf:Clients + folder:Folders/Projects.
LABELLED='<labelled-0006@threestrands.test>'
for box in "INBOX" "Folders/Projects" "Labels/Clients"; do
  deliver "$box" <<EOF
From: eve@example.test
To: test@threestrands.test
Subject: A labelled and foldered message
Message-ID: ${LABELLED}
Date: Mon, 06 Oct 2025 12:00:00 +0000

One message in INBOX, a user folder and a label folder — three locations.
EOF
done

# An OLD message that exists ONLY in the user folder (no INBOX copy): the live
# test asserts it is index-synced but NOT reported (never hot — no INBOX/Sent
# location) and does not surface its body.
deliver "Folders/Projects" <<'EOF'
From: frank@example.test
To: test@threestrands.test
Subject: An old project note
Message-ID: <folder-only-0007@example.test>
Date: Sun, 01 Jun 2025 08:00:00 +0000

An older message that lives only in the user folder.
EOF

touch "$MARKER"
echo "[seed] done"
