#!/bin/sh
# Seed deterministic test mail for the IMAP integration tests. Idempotent: a
# marker file guards re-seeding so container restarts keep stable UIDs.
#
# Messages are delivered with dovecot-lda so they land as real Maildir files
# with Dovecot-assigned UIDs. The set is intentionally small and fixed:
#   - 2 INBOX messages (one plain, one with a Message-ID we also drop into Sent
#     to exercise the "one message, two locations" identity rule);
#   - 1 Sent message (the shared Message-ID);
#   - 1 Archive message.
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

touch "$MARKER"
echo "[seed] done"
