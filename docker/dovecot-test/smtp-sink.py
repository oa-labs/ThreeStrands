#!/usr/bin/env python3
# A tiny SMTP SINK for the Phase 4 SMTP submission live tests. Not an MTA: it
# only authenticates the single test user, accepts a message and writes it to a
# file the Rust live test can read back, so a send can be asserted byte for
# byte. Stdlib only (asyncio + ssl); the Ubuntu 24.04 base ships Python 3.12,
# and `aiosmtpd`/`smtpd` are NOT stdlib, so this is hand-rolled and dependency
# free, keeping the image build as offline-friendly as the apt step allows.
#
# It uses the SAME self-signed cert as the IMAP side (/etc/dovecot/ssl), so the
# client fingerprint-pins ONE certificate for both protocols. It serves:
#   - implicit TLS on IMPLICIT_PORT (default 1465), and
#   - STARTTLS on STARTTLS_PORT (default 1587),
# advertises AUTH PLAIN LOGIN, and accepts the test user's credentials only.
#
# Received messages are written to RECEIVED_DIR as <n>.eml plus a <n>.meta
# file with the envelope. The newest message is also copied to latest.eml so a
# test does not have to guess the sequence number.

import asyncio
import base64
import os
import ssl
import sys

HOST = "0.0.0.0"
IMPLICIT_PORT = int(os.environ.get("SMTP_SINK_IMPLICIT_PORT", "1465"))
STARTTLS_PORT = int(os.environ.get("SMTP_SINK_STARTTLS_PORT", "1587"))
CERT = os.environ.get("SMTP_SINK_CERT", "/etc/dovecot/ssl/dovecot.pem")
KEY = os.environ.get("SMTP_SINK_KEY", "/etc/dovecot/ssl/dovecot.key")
RECEIVED_DIR = os.environ.get("SMTP_SINK_RECEIVED", "/srv/smtp-sink/received")
USER = os.environ.get("SMTP_SINK_USER", "test@threestrands.test")
PASSWORD = os.environ.get("SMTP_SINK_PASSWORD", "testpassword")

_counter = 0


def tls_context() -> ssl.SSLContext:
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(CERT, KEY)
    return ctx


def _check_plain(blob: str) -> bool:
    # AUTH PLAIN payload is base64 of "\0user\0pass".
    try:
        decoded = base64.b64decode(blob).split(b"\x00")
    except Exception:
        return False
    if len(decoded) != 3:
        return False
    _authzid, user, pw = decoded
    return user.decode("utf-8", "replace") == USER and pw.decode("utf-8", "replace") == PASSWORD


async def handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter, start_tls: bool):
    global _counter
    ctx = tls_context()

    async def send(line: str):
        writer.write(line.encode() + b"\r\n")
        await writer.drain()

    async def readline() -> bytes:
        return await reader.readline()

    await send("220 sink ready")
    authed = False
    mail_from = None
    rcpts = []

    while True:
        raw = await readline()
        if not raw:
            break
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        up = line.upper()
        if up.startswith("EHLO") or up.startswith("HELO"):
            if start_tls:
                await send("250-sink")
                await send("250-STARTTLS")
                await send("250 AUTH PLAIN LOGIN")
            else:
                await send("250-sink")
                await send("250 AUTH PLAIN LOGIN")
        elif up == "STARTTLS":
            if not start_tls:
                await send("502 already in TLS")
                continue
            await send("220 go ahead")
            # Upgrade the transport in place.
            transport = writer.transport
            protocol = transport.get_protocol()
            loop = asyncio.get_event_loop()
            new_transport = await loop.start_tls(transport, protocol, ctx, server_side=True)
            protocol._stream_reader._transport = new_transport  # type: ignore[attr-defined]
            writer._transport = new_transport  # type: ignore[attr-defined]
            start_tls = False  # already upgraded now
        elif up.startswith("AUTH LOGIN"):
            await send("334 VXNlcm5hbWU6")  # "Username:"
            await readline()  # username (ignored; checked at password step below)
            await send("334 UGFzc3dvcmQ6")  # "Password:"
            pw_line = (await readline()).decode("utf-8", "replace").strip()
            try:
                ok = base64.b64decode(pw_line).decode("utf-8", "replace") == PASSWORD
            except Exception:
                ok = False
            if ok:
                authed = True
                await send("235 2.7.0 authenticated")
            else:
                await send("535 5.7.8 bad credentials")
        elif up.startswith("AUTH PLAIN"):
            parts = line.split(" ", 2)
            if len(parts) == 3:
                authed = _check_plain(parts[2])
                await send("235 2.7.0 authenticated" if authed else "535 5.7.8 bad credentials")
            else:
                await send("334 ")
                blob = (await readline()).decode("utf-8", "replace").strip()
                authed = _check_plain(blob)
                await send("235 2.7.0 authenticated" if authed else "535 5.7.8 bad credentials")
        elif up.startswith("MAIL FROM"):
            if not authed:
                await send("530 5.7.0 authentication required")
                continue
            mail_from = line
            rcpts = []
            await send("250 2.1.0 ok")
        elif up.startswith("RCPT TO"):
            if not authed:
                await send("530 5.7.0 authentication required")
                continue
            rcpts.append(line)
            await send("250 2.1.5 ok")
        elif up == "DATA":
            await send("354 end with <CRLF>.<CRLF>")
            body = bytearray()
            while True:
                chunk = await readline()
                if not chunk:
                    break
                body.extend(chunk)
                if body.endswith(b"\r\n.\r\n") or body == b".\r\n":
                    break
            os.makedirs(RECEIVED_DIR, exist_ok=True)
            _counter += 1
            base = os.path.join(RECEIVED_DIR, str(_counter))
            # Strip the trailing dot terminator for the stored body.
            stored = bytes(body)
            if stored.endswith(b"\r\n.\r\n"):
                stored = stored[: -len(b"\r\n.\r\n")] + b"\r\n"
            with open(base + ".eml", "wb") as fh:
                fh.write(stored)
            with open(base + ".meta", "w") as fh:
                fh.write((mail_from or "") + "\n")
                fh.write("\n".join(rcpts) + "\n")
            with open(os.path.join(RECEIVED_DIR, "latest.eml"), "wb") as fh:
                fh.write(stored)
            await send("250 2.0.0 message accepted for delivery")
        elif up == "RSET":
            mail_from = None
            rcpts = []
            await send("250 2.0.0 reset")
        elif up == "NOOP":
            await send("250 2.0.0 ok")
        elif up == "QUIT":
            await send("221 2.0.0 bye")
            break
        else:
            await send("250 ok")

    try:
        writer.close()
    except Exception:
        pass


async def main():
    implicit_ctx = tls_context()
    implicit = await asyncio.start_server(
        lambda r, w: handle(r, w, start_tls=False), HOST, IMPLICIT_PORT, ssl=implicit_ctx
    )
    starttls = await asyncio.start_server(
        lambda r, w: handle(r, w, start_tls=True), HOST, STARTTLS_PORT
    )
    print(f"[smtp-sink] implicit TLS on {IMPLICIT_PORT}, STARTTLS on {STARTTLS_PORT}", flush=True)
    async with implicit, starttls:
        await asyncio.gather(implicit.serve_forever(), starttls.serve_forever())


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        sys.exit(0)
