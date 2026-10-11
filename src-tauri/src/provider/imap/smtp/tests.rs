//! Tests for the SMTP submission client and the moved setup dry-run.
//!
//! The centrepiece is [`FakeSmtp`]: a scripted TLS SMTP server that, per test,
//! injects a failure at a chosen phase AND records the exact command lines and
//! body bytes it received. Every delivery test asserts BOTH the outcome class
//! (Accepted / NotDelivered / Unknown) and what reached the wire, so a test
//! can never pass vacuously (the failure past runs hit: green tests that could
//! not actually inject the failure they claimed).

use super::*;
use crate::provider::imap::settings::SecurityMode;
use crate::provider::imap::tls::Sha256Fingerprint;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Scripted fake SMTP server.
// ---------------------------------------------------------------------------

/// What the fake server should do, chosen per test. Each variant injects at a
/// distinct phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Script {
    /// Full happy path, 250 to everything, 250 final.
    Accept,
    /// Refuse the TCP connection (nothing listens). Driven by using a dead
    /// port, not this server.
    RefuseConnect,
    /// STARTTLS mode but the EHLO before TLS does NOT advertise STARTTLS.
    NoStartTls,
    /// 535 on AUTH.
    Reject535Auth,
    /// 553 on MAIL FROM.
    Reject553MailFrom,
    /// 550 on the SECOND of two RCPTs (first ok).
    Reject550SecondRcpt,
    /// 450 (transient) on the first RCPT.
    Reject450Rcpt,
    /// Drop the connection right after RCPT ok, before answering DATA's 354.
    DropBefore354,
    /// 554 reply to DATA (before 354).
    Reject554ToData,
    /// 354, then drop mid-body (close before the terminator).
    DropMidBody,
    /// 354, full body received, then drop with NO reply.
    DropAfterDot,
    /// 354, full body received, then never reply (stall).
    StallAfterDot,
    /// 354, full body received, 451 transient final reply.
    Reject451Final,
    /// 354, full body received, 554 permanent final reply.
    Reject554Final,
    /// A multi-line 250 final reply (continuation lines).
    MultilineFinal,
    /// A garbage / non-numeric reply line after the body (protocol error).
    GarbageFinal,
    /// A final reply whose text carries CR/LF/control characters to sanitise.
    ControlCharsFinal,
    /// A very long final reply line (text capped).
    VeryLongFinal,
    /// 250 success, used by implicit-TLS tests.
    AcceptImplicit,
    /// MAIL FROM is answered 250 only after a delay longer than the CONNECT
    /// timeout but shorter than the COMMAND timeout (review: the command
    /// timeout must govern envelope commands, not the connect timeout).
    SlowMailFrom,
    /// The final 250 arrives after a delay longer than the CONNECT and
    /// COMMAND timeouts but shorter than the FINAL-REPLY timeout (a slow but
    /// legitimate MTA must be Accepted, not reported "may have been sent").
    SlowFinal,
    /// A non-2xx/4xx/5xx final reply (a stray 354) after the body: the server
    /// has not given a verdict, so this must be Unknown, never definite.
    Odd3xxFinal,
}

struct FakeSmtp {
    port: u16,
    pin: Sha256Fingerprint,
    log: Arc<Mutex<Vec<String>>>,
    body: Arc<Mutex<Vec<u8>>>,
}

impl FakeSmtp {
    fn commands(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn body_bytes(&self) -> Vec<u8> {
        self.body.lock().unwrap().clone()
    }
    fn saw(&self, needle: &str) -> bool {
        self.commands().iter().any(|l| l.contains(needle))
    }
    fn saw_prefix(&self, prefix: &str) -> bool {
        self.commands()
            .iter()
            .any(|l| l.to_ascii_uppercase().starts_with(prefix))
    }
}

/// Start a scripted TLS SMTP server. `implicit` = implicit TLS (wrap socket
/// first); otherwise STARTTLS. Returns once it is listening.
async fn start(script: Script, implicit: bool) -> FakeSmtp {
    use rustls::pki_types::PrivatePkcs8KeyDer;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let pin = Sha256Fingerprint::of_certificate(cert.cert.der());
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let body = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    let body2 = body.clone();
    tokio::spawn(async move {
        let (tcp, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => return,
        };
        if implicit {
            let Ok(tls) = acceptor.accept(tcp).await else {
                return;
            };
            run_session(tls, script, log2, body2, true).await;
        } else {
            // STARTTLS: greet, read EHLO, advertise (or not) STARTTLS, read
            // STARTTLS, upgrade, then run the TLS session.
            let mut tcp = tcp;
            if tcp.write_all(b"220 fake ready\r\n").await.is_err() {
                return;
            }
            // Read EHLO (plaintext).
            let mut reader = BufReader::new(&mut tcp);
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            log2.lock().unwrap().push(line.trim_end().to_string());
            drop(reader);
            if script == Script::NoStartTls {
                // Advertise AUTH but NOT STARTTLS; mail-send must refuse.
                let _ = tcp
                    .write_all(b"250-fake\r\n250 AUTH PLAIN LOGIN\r\n")
                    .await;
                // The client should give up here (MissingStartTls); drain.
                let mut buf = [0u8; 64];
                let _ = tcp.read(&mut buf).await;
                return;
            }
            let _ = tcp.write_all(b"250-fake\r\n250 STARTTLS\r\n").await;
            // Read STARTTLS.
            let mut reader = BufReader::new(&mut tcp);
            line.clear();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            log2.lock().unwrap().push(line.trim_end().to_string());
            drop(reader);
            let _ = tcp.write_all(b"220 go ahead\r\n").await;
            let Ok(tls) = acceptor.accept(tcp).await else {
                return;
            };
            run_session(tls, script, log2, body2, false).await;
        }
    });
    FakeSmtp { port, pin, log, body }
}

/// Drive the SMTP conversation over the (already TLS) stream per the script.
async fn run_session<S>(
    stream: S,
    script: Script,
    log: Arc<Mutex<Vec<String>>>,
    body: Arc<Mutex<Vec<u8>>>,
    greet: bool,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut s = BufReader::new(stream);
    if greet {
        if s.write_all(b"220 fake ready\r\n").await.is_err() {
            return;
        }
    }
    let mut rcpt_seen = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        let n = match s.read_line(&mut line).await {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            return;
        }
        let trimmed = line.trim_end().to_string();
        log.lock().unwrap().push(trimmed.clone());
        let up = trimmed.to_ascii_uppercase();
        if up.starts_with("EHLO") {
            let _ = s
                .write_all(b"250-fake\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n")
                .await;
        } else if up.starts_with("AUTH") {
            // mail-send sends AUTH PLAIN with the SASL payload inline, or
            // LOGIN. Accept either; reply per script.
            if script == Script::Reject535Auth {
                let _ = s.write_all(b"535 5.7.8 bad credentials\r\n").await;
                // The client aborts; drain and close.
                return;
            }
            // AUTH LOGIN needs two 334 challenges; AUTH PLAIN finishes at once.
            if up.starts_with("AUTH LOGIN") {
                let _ = s.write_all(b"334 VXNlcm5hbWU6\r\n").await;
                // username
                line.clear();
                if s.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let _ = s.write_all(b"334 UGFzc3dvcmQ6\r\n").await;
                // password
                line.clear();
                if s.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let _ = s.write_all(b"235 2.7.0 authenticated\r\n").await;
            } else {
                // AUTH PLAIN (inline).
                let _ = s.write_all(b"235 2.7.0 authenticated\r\n").await;
            }
        } else if up.starts_with("MAIL FROM") {
            if script == Script::Reject553MailFrom {
                let _ = s.write_all(b"553 5.7.1 sender address rejected\r\n").await;
            } else if script == Script::SlowMailFrom {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                let _ = s.write_all(b"250 2.1.0 ok\r\n").await;
            } else {
                let _ = s.write_all(b"250 2.1.0 ok\r\n").await;
            }
        } else if up.starts_with("RCPT TO") {
            rcpt_seen += 1;
            match script {
                Script::Reject450Rcpt => {
                    let _ = s.write_all(b"450 4.2.1 mailbox busy, try later\r\n").await;
                }
                Script::Reject550SecondRcpt if rcpt_seen == 2 => {
                    let _ = s.write_all(b"550 5.1.1 no such user\r\n").await;
                }
                _ => {
                    let _ = s.write_all(b"250 2.1.5 ok\r\n").await;
                }
            }
        } else if up == "DATA" {
            match script {
                Script::DropBefore354 => return,
                Script::Reject554ToData => {
                    let _ = s.write_all(b"554 5.7.1 rejected before data\r\n").await;
                    continue;
                }
                _ => {
                    let _ = s.write_all(b"354 end with <CRLF>.<CRLF>\r\n").await;
                }
            }
            // Read the body until the terminating "\r\n.\r\n", recording bytes.
            let mut collected: Vec<u8> = Vec::new();
            let mut buf = [0u8; 256];
            loop {
                let m = match s.read(&mut buf).await {
                    Ok(m) => m,
                    Err(_) => return,
                };
                if m == 0 {
                    // EOF mid-body.
                    *body.lock().unwrap() = collected;
                    return;
                }
                collected.extend_from_slice(&buf[..m]);
                if script == Script::DropMidBody && collected.len() >= 8 {
                    *body.lock().unwrap() = collected;
                    return; // vanish before the terminator
                }
                if collected.ends_with(b"\r\n.\r\n") || collected == b".\r\n" {
                    break;
                }
            }
            *body.lock().unwrap() = collected;
            match script {
                Script::DropAfterDot => return,
                Script::StallAfterDot => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    return;
                }
                Script::SlowFinal => {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let _ = s.write_all(b"250 2.0.0 accepted late\r\n").await;
                }
                Script::Odd3xxFinal => {
                    let _ = s.write_all(b"354 unexpected continuation\r\n").await;
                }
                Script::Reject451Final => {
                    let _ = s.write_all(b"451 4.3.0 try again later\r\n").await;
                }
                Script::Reject554Final => {
                    let _ = s.write_all(b"554 5.7.1 message rejected as spam\r\n").await;
                }
                Script::MultilineFinal => {
                    let _ = s
                        .write_all(b"250-queued\r\n250-still going\r\n250 2.0.0 accepted\r\n")
                        .await;
                }
                Script::GarbageFinal => {
                    let _ = s.write_all(b"not an smtp reply at all\r\n").await;
                }
                Script::ControlCharsFinal => {
                    // Embedded control chars in the (final) reply text.
                    let _ = s
                        .write_all(b"250 2.0.0 ok\x01\x02 done\twith\x07bells\r\n")
                        .await;
                }
                Script::VeryLongFinal => {
                    let mut long = b"250 2.0.0 ".to_vec();
                    long.extend(std::iter::repeat(b'x').take(4000));
                    long.extend_from_slice(b"\r\n");
                    let _ = s.write_all(&long).await;
                }
                _ => {
                    let _ = s.write_all(b"250 2.0.0 message accepted for delivery\r\n").await;
                }
            }
        } else if up == "RSET" {
            let _ = s.write_all(b"250 2.0.0 reset\r\n").await;
        } else if up == "QUIT" {
            let _ = s.write_all(b"221 2.0.0 bye\r\n").await;
            return;
        } else {
            let _ = s.write_all(b"250 ok\r\n").await;
        }
    }
}

fn endpoint(srv: &FakeSmtp, implicit: bool, pin: Option<Sha256Fingerprint>) -> SmtpEndpoint {
    SmtpEndpoint {
        host: "127.0.0.1".into(),
        port: srv.port,
        security: if implicit {
            SecurityMode::ImplicitTls
        } else {
            SecurityMode::StartTls
        },
        pinned: pin.or(Some(srv.pin)),
    }
}

fn creds() -> SmtpCredentials {
    SmtpCredentials {
        username: "user".into(),
        password: "secret".into(),
    }
}

/// Short timeouts so the stall case is fast under real (not paused) time.
fn fast_timeouts() -> SmtpTimeouts {
    SmtpTimeouts {
        connect: Duration::from_secs(5),
        command: Duration::from_secs(2),
        final_reply: Duration::from_millis(400),
    }
}

const BODY: &[u8] = b"From: a@b.example\r\nTo: c@d.example\r\nSubject: hi\r\n\r\nhello world\r\n";

async fn connect(srv: &FakeSmtp, implicit: bool) -> Result<SmtpSession, SmtpFailure> {
    SmtpSession::connect_with_timeouts(&endpoint(srv, implicit, None), &creds(), fast_timeouts())
        .await
}

// ---------------------------------------------------------------------------
// Happy path, both TLS modes: Accepted + exact wire bytes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accepts_and_sends_the_exact_framed_bytes_in_both_tls_modes() {
    for implicit in [false, true] {
        let script = if implicit { Script::AcceptImplicit } else { Script::Accept };
        let srv = start(script, implicit).await;
        let session = connect(&srv, implicit).await.expect("prepare phase succeeds");
        let out = session
            .send("a@b.example", &["c@d.example".to_string()], BODY)
            .await;
        match out {
            SmtpOutcome::Accepted { reply } => assert!(is_2xx(reply.code)),
            other => panic!("{implicit} expected Accepted, got {other:?}"),
        }
        // The envelope and the framed body reached the wire exactly.
        assert!(srv.saw("MAIL FROM:<a@b.example>"), "{implicit}");
        assert!(srv.saw("RCPT TO:<c@d.example>"), "{implicit}");
        let body = srv.body_bytes();
        let expected = frame_message(BODY);
        assert_eq!(body, expected, "{implicit} body bytes must match frame_message exactly");
        assert!(body.ends_with(b"\r\n.\r\n"), "{implicit}");
    }
}

// ---------------------------------------------------------------------------
// Wrong pin fails in the handshake, BEFORE any AUTH on the wire.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_wrong_pin_fails_before_auth_in_both_modes() {
    for implicit in [false, true] {
        let script = if implicit { Script::AcceptImplicit } else { Script::Accept };
        let srv = start(script, implicit).await;
        let wrong = Sha256Fingerprint::from_bytes([0xEE; 32]);
        let res = SmtpSession::connect_with_timeouts(
            &endpoint(&srv, implicit, Some(wrong)),
            &creds(),
            fast_timeouts(),
        )
        .await;
        let fail = res.err().expect("wrong pin must fail connect");
        assert_eq!(fail.phase, Phase::Prepare);
        assert!(
            matches!(fail.error, ProviderError::TransientTransport(_)),
            "{implicit} wrong pin is transport-class, got {:?}",
            fail.error
        );
        // No AUTH bytes ever reached the server.
        assert!(!srv.saw_prefix("AUTH"), "{implicit} AUTH must not reach the wire on a wrong pin");
    }
}

// ---------------------------------------------------------------------------
// STARTTLS not offered => fail before AUTH, no plaintext path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn starttls_mode_with_no_starttls_offered_fails_before_auth() {
    let srv = start(Script::NoStartTls, false).await;
    let res = connect(&srv, false).await;
    let fail = res.err().expect("must refuse when STARTTLS is absent");
    assert_eq!(fail.phase, Phase::Prepare);
    assert!(matches!(fail.error, ProviderError::TransientTransport(_)));
    // EHLO was sent plaintext (allowed), but AUTH never was.
    assert!(srv.saw_prefix("EHLO"));
    assert!(!srv.saw_prefix("AUTH"), "no credentials in the clear");
}

// ---------------------------------------------------------------------------
// 535 on AUTH => reauth class, before any message bytes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_535_maps_to_reauthentication_required() {
    let srv = start(Script::Reject535Auth, false).await;
    let res = connect(&srv, false).await;
    let fail = res.err().expect("535 on AUTH must fail connect");
    assert_eq!(fail.phase, Phase::Prepare);
    assert!(
        fail.error.requires_reauthentication(),
        "535 must map to the reauth class, got {:?}",
        fail.error
    );
    assert!(!srv.saw_prefix("MAIL FROM"), "no envelope after an auth failure");
}

// ---------------------------------------------------------------------------
// The E-case table: per phase, assert the outcome class AND the wire bytes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mail_from_553_is_permanent_not_delivered() {
    let srv = start(Script::Reject553MailFrom, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::NotDelivered(f) => {
            assert_eq!(f.phase, Phase::MailFrom);
            assert_eq!(f.reply.as_ref().unwrap().code, 553);
            assert!(matches!(f.error, ProviderError::PermanentClientRejection(_)));
        }
        other => panic!("expected NotDelivered, got {other:?}"),
    }
    assert!(srv.saw("MAIL FROM:<a@b.example>"));
    // No RCPT or body followed a rejected sender.
    assert!(!srv.saw_prefix("RCPT TO"));
    assert!(srv.body_bytes().is_empty());
}

#[tokio::test]
async fn any_rejected_rcpt_aborts_the_whole_send_naming_the_address() {
    let srv = start(Script::Reject550SecondRcpt, false).await;
    let rcpts = vec!["first@x.example".to_string(), "second@y.example".to_string()];
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &rcpts, BODY)
        .await;
    match out {
        SmtpOutcome::NotDelivered(f) => {
            assert_eq!(f.phase, Phase::Rcpt);
            assert_eq!(f.reply.as_ref().unwrap().code, 550);
            match &f.error {
                ProviderError::PermanentClientRejection(m) => {
                    assert!(m.contains("second@y.example"), "must name the rejected address: {m}");
                }
                other => panic!("expected permanent, got {other:?}"),
            }
        }
        other => panic!("expected NotDelivered, got {other:?}"),
    }
    // Both RCPTs were attempted, but NO body was sent (never a silent subset).
    assert!(srv.saw("RCPT TO:<first@x.example>"));
    assert!(srv.saw("RCPT TO:<second@y.example>"));
    assert!(srv.body_bytes().is_empty(), "no DATA after a rejected recipient");
}

#[tokio::test]
async fn rcpt_450_is_transient_not_delivered() {
    let srv = start(Script::Reject450Rcpt, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::NotDelivered(f) => {
            assert_eq!(f.phase, Phase::Rcpt);
            assert_eq!(f.reply.as_ref().unwrap().code, 450);
            assert!(
                matches!(f.error, ProviderError::TransientTransport(_)),
                "a 4xx RCPT is transient-class, got {:?}",
                f.error
            );
            assert!(f.error.retry_mutation(), "transient must be retryable");
        }
        other => panic!("expected NotDelivered, got {other:?}"),
    }
    assert!(srv.body_bytes().is_empty());
}

#[tokio::test]
async fn drop_before_354_is_definite_not_delivered() {
    let srv = start(Script::DropBefore354, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::NotDelivered(f) => {
            assert_eq!(f.phase, Phase::DataRequest);
            assert!(matches!(f.error, ProviderError::TransientTransport(_)));
        }
        other => panic!("expected NotDelivered, got {other:?}"),
    }
    assert!(srv.saw("DATA"));
    assert!(srv.body_bytes().is_empty(), "no body bytes reached the server");
}

#[tokio::test]
async fn data_554_before_body_is_permanent_not_delivered() {
    let srv = start(Script::Reject554ToData, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::NotDelivered(f) => {
            assert_eq!(f.phase, Phase::DataRequest);
            assert_eq!(f.reply.as_ref().unwrap().code, 554);
            assert!(matches!(f.error, ProviderError::PermanentClientRejection(_)));
        }
        other => panic!("expected NotDelivered, got {other:?}"),
    }
    assert!(srv.body_bytes().is_empty());
}

#[tokio::test]
async fn drop_mid_body_is_unknown() {
    let srv = start(Script::DropMidBody, false).await;
    // A long body so the server can read some, then vanish mid-stream.
    let big = BODY.repeat(80);
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], &big)
        .await;
    // The server dropped mid-body. Depending on OS socket buffering our
    // `write_all` may itself error (phase Body) or may succeed into a buffer
    // and only the subsequent final-reply read sees the EOF (phase
    // FinalReply). BOTH are in the UNKNOWN zone, which is the load-bearing
    // guarantee — a mid-body drop must NEVER be reported as a definite
    // failure, because the server may hold a prefix of the message. The exact
    // phase label inside the unknown zone is not a contract.
    match out {
        SmtpOutcome::Unknown(u) => {
            assert!(
                u.phase.is_unknown_on_failure(),
                "a mid-body drop must stay in the unknown zone, got {:?}",
                u.phase
            );
        }
        other => panic!("drop mid-body must be Unknown (conservative), got {other:?}"),
    }
}

#[tokio::test]
async fn drop_after_dot_with_no_reply_is_unknown() {
    let srv = start(Script::DropAfterDot, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::Unknown(u) => assert_eq!(u.phase, Phase::FinalReply),
        other => panic!("drop after '.' with no reply must be Unknown, got {other:?}"),
    }
    // The whole body DID reach the server, which is why this is Unknown.
    assert_eq!(srv.body_bytes(), frame_message(BODY));
}

#[tokio::test]
async fn stall_waiting_for_final_reply_is_unknown_and_fast() {
    let srv = start(Script::StallAfterDot, false).await;
    let started = std::time::Instant::now();
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::Unknown(u) => assert_eq!(u.phase, Phase::FinalReply),
        other => panic!("a hung final reply must be Unknown, got {other:?}"),
    }
    // The final-reply timeout (400ms) fired; we did not wait anywhere near the
    // server's 3600s stall.
    assert!(started.elapsed() < Duration::from_secs(10), "must give up on our own timeout");
}

/// Timeouts where CONNECT < COMMAND < FINAL-REPLY, all generous enough for a
/// local handshake, so each phase's own ceiling is the thing under test.
fn staged_timeouts() -> SmtpTimeouts {
    SmtpTimeouts {
        connect: Duration::from_millis(1000),
        command: Duration::from_millis(4000),
        final_reply: Duration::from_millis(6000),
    }
}

#[tokio::test]
async fn a_slow_envelope_reply_is_governed_by_the_command_timeout_not_the_connect_one() {
    let srv = start(Script::SlowMailFrom, false).await;
    let session = SmtpSession::connect_with_timeouts(
        &endpoint(&srv, false, None),
        &creds(),
        staged_timeouts(),
    )
    .await
    .unwrap();
    let out = session
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    assert!(
        matches!(out, SmtpOutcome::Accepted { .. }),
        "a MAIL FROM reply slower than the connect timeout but within the command timeout must succeed, got {out:?}"
    );
}

#[tokio::test]
async fn a_slow_final_reply_within_its_own_timeout_is_accepted() {
    let srv = start(Script::SlowFinal, false).await;
    let session = SmtpSession::connect_with_timeouts(
        &endpoint(&srv, false, None),
        &creds(),
        staged_timeouts(),
    )
    .await
    .unwrap();
    let out = session
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    assert!(
        matches!(out, SmtpOutcome::Accepted { .. }),
        "a final reply slower than connect/command timeouts but within final_reply must be Accepted, got {out:?}"
    );
}

#[tokio::test]
async fn a_non_verdict_final_reply_is_unknown_not_definite() {
    let srv = start(Script::Odd3xxFinal, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::Unknown(u) => assert_eq!(u.phase, Phase::FinalReply),
        other => panic!("a 1xx/3xx reply after the body is no verdict and must be Unknown, got {other:?}"),
    }
}

#[tokio::test]
async fn final_451_is_transient_not_delivered() {
    let srv = start(Script::Reject451Final, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::NotDelivered(f) => {
            assert_eq!(f.phase, Phase::FinalReply);
            assert_eq!(f.reply.as_ref().unwrap().code, 451);
            assert!(
                matches!(f.error, ProviderError::TransientTransport(_)),
                "a 4xx final reply is transient, got {:?}",
                f.error
            );
        }
        other => panic!("expected NotDelivered, got {other:?}"),
    }
    // The full body reached the server, yet the final 4xx makes it DEFINITE.
    assert_eq!(srv.body_bytes(), frame_message(BODY));
}

#[tokio::test]
async fn final_554_is_permanent_not_delivered() {
    let srv = start(Script::Reject554Final, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::NotDelivered(f) => {
            assert_eq!(f.phase, Phase::FinalReply);
            assert_eq!(f.reply.as_ref().unwrap().code, 554);
            assert!(matches!(f.error, ProviderError::PermanentClientRejection(_)));
        }
        other => panic!("expected NotDelivered, got {other:?}"),
    }
}

#[tokio::test]
async fn multiline_final_reply_is_accepted() {
    let srv = start(Script::MultilineFinal, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::Accepted { reply } => {
            assert_eq!(reply.code, 250);
            assert!(reply.text.contains("accepted"), "last line's text is kept: {}", reply.text);
        }
        other => panic!("a multi-line 250 must be Accepted, got {other:?}"),
    }
}

#[tokio::test]
async fn garbage_final_reply_is_classified_by_phase_never_panics() {
    let srv = start(Script::GarbageFinal, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    // An unparseable reply after the body is read as a read error => Unknown
    // (the body WAS sent). The point is it does not panic and stays safe.
    match out {
        SmtpOutcome::Unknown(u) => assert_eq!(u.phase, Phase::FinalReply),
        other => panic!("garbage final reply must be a safe Unknown, got {other:?}"),
    }
}

#[tokio::test]
async fn control_chars_in_final_reply_are_sanitised() {
    let srv = start(Script::ControlCharsFinal, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::Accepted { reply } => {
            assert!(!reply.text.contains('\x01'));
            assert!(!reply.text.contains('\x07'));
            assert!(!reply.text.contains('\t'));
            assert!(!reply.text.contains('\r'));
            assert!(!reply.text.contains('\n'));
        }
        other => panic!("expected Accepted with sanitised text, got {other:?}"),
    }
}

#[tokio::test]
async fn very_long_final_reply_text_is_capped() {
    let srv = start(Script::VeryLongFinal, false).await;
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::Accepted { reply } => {
            assert!(
                reply.text.len() <= policy::MAX_SMTP_REPLY_TEXT_BYTES,
                "reply text must be capped, was {} bytes",
                reply.text.len()
            );
        }
        other => panic!("expected Accepted, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Connection refused (nothing listening) is a DEFINITE failure.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connection_refused_is_definite_transient_transport() {
    // Bind then drop a listener to get a port nothing is listening on.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = listener.local_addr().unwrap().port();
    drop(listener);
    let endpoint = SmtpEndpoint {
        host: "127.0.0.1".into(),
        port: dead_port,
        security: SecurityMode::ImplicitTls,
        pinned: Some(Sha256Fingerprint::from_bytes([0x11; 32])),
    };
    let res =
        SmtpSession::connect_with_timeouts(&endpoint, &creds(), fast_timeouts()).await;
    let fail = res.err().expect("a refused connection must fail");
    assert_eq!(fail.phase, Phase::Prepare);
    assert!(matches!(fail.error, ProviderError::TransientTransport(_)));
}

// ---------------------------------------------------------------------------
// Security-negative: address validation rejects injection BEFORE any I/O.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_crlf_injection_address_sends_zero_bytes() {
    let srv = start(Script::Accept, false).await;
    // The spike proved mail-send itself does NOT reject this; WE must.
    let evil = "a@b.example>\r\nRCPT TO:<attacker@evil.example".to_string();
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send(&evil, &["c@d.example".to_string()], BODY)
        .await;
    match out {
        SmtpOutcome::NotDelivered(f) => {
            assert_eq!(f.phase, Phase::MailFrom);
            assert!(matches!(f.error, ProviderError::PermanentClientRejection(_)));
        }
        other => panic!("an injection address must be NotDelivered, got {other:?}"),
    }
    // CRITICAL: nothing from the envelope ever reached the wire.
    assert!(!srv.saw_prefix("MAIL FROM"), "no MAIL FROM on the wire");
    assert!(!srv.saw("attacker@evil.example"), "injected RCPT must never reach the server");
    assert!(srv.body_bytes().is_empty());
}

#[tokio::test]
async fn an_injection_in_a_recipient_sends_zero_envelope_bytes() {
    let srv = start(Script::Accept, false).await;
    let evil_rcpt = "c@d.example>\r\nDATA\r\nSmuggled".to_string();
    let out = connect(&srv, false)
        .await
        .unwrap()
        .send("a@b.example", &[evil_rcpt], BODY)
        .await;
    assert!(matches!(out, SmtpOutcome::NotDelivered(_)));
    assert!(!srv.saw_prefix("MAIL FROM"), "validation runs before ANY command");
    assert!(!srv.saw_prefix("RCPT TO"));
    assert!(!srv.saw("Smuggled"));
}

// ---------------------------------------------------------------------------
// Pure-function tests: address validation, framing, phase classification.
// ---------------------------------------------------------------------------

#[test]
fn address_validation_accepts_ordinary_and_rejects_the_dangerous_forms() {
    for ok in ["a@b.example", "first.last+tag@sub.domain.example", "x@y"] {
        assert!(validate_envelope_address(ok).is_ok(), "{ok} should be valid");
    }
    for bad in [
        "",                                    // empty
        "no-at-sign",                          // no @
        "a@@b",                                // two @
        "@domain.example",                     // empty local
        "local@",                              // empty domain
        "a@b.example>\r\nRCPT TO:<x@y",        // CRLF injection
        "a@b\nexample",                        // lone LF
        "a @b.example",                        // space
        "a\t@b.example",                       // tab
        "a<x>@b.example",                      // angle brackets
        "a\0@b.example",                       // NUL
        "üser@domain.example",                 // non-ASCII local (no SMTPUTF8)
    ] {
        let err = validate_envelope_address(bad).unwrap_err();
        assert!(
            matches!(err, ProviderError::PermanentClientRejection(_)),
            "{bad:?} must be a permanent rejection, got {err:?}"
        );
    }
    // Exactly at the length limit is ok; one over is rejected.
    let local_len = policy::MAX_SMTP_ADDRESS_BYTES - "@b.example".len();
    let at_limit = format!("{}@b.example", "a".repeat(local_len));
    assert_eq!(at_limit.len(), policy::MAX_SMTP_ADDRESS_BYTES);
    assert!(validate_envelope_address(&at_limit).is_ok());
    let over = format!("{}@b.example", "a".repeat(local_len + 1));
    assert!(validate_envelope_address(&over).is_err());
}

#[test]
fn frame_message_normalises_crlf_and_dot_stuffs_every_leading_dot() {
    // Byte 0 is a dot: it MUST be stuffed (mail-send's own writer would not).
    assert_eq!(frame_message(b".hello\r\n"), b"..hello\r\n.\r\n");
    // A line that is exactly "." is stuffed to "..".
    assert_eq!(frame_message(b"a\r\n.\r\n"), b"a\r\n..\r\n.\r\n");
    // A lone LF is normalised to CRLF, and a following leading dot is stuffed.
    assert_eq!(frame_message(b"a\n.b"), b"a\r\n..b\r\n.\r\n");
    // A lone CR is normalised to CRLF.
    assert_eq!(frame_message(b"a\rb"), b"a\r\nb\r\n.\r\n");
    // A message ending without a trailing CRLF still gets the terminator with
    // a CRLF inserted before the dot line.
    assert_eq!(frame_message(b"no newline"), b"no newline\r\n.\r\n");
    // Existing CRLF is preserved (idempotent), not doubled.
    assert_eq!(frame_message(b"a\r\nb\r\n"), b"a\r\nb\r\n.\r\n");
    // A mid-line dot (not at a line start) is left alone.
    assert_eq!(frame_message(b"a.b\r\n"), b"a.b\r\n.\r\n");
    // Every frame ends with the SMTP data terminator.
    for input in [&b""[..], b".", b"\n", b"x", b"...\r\n"] {
        assert!(frame_message(input).ends_with(b"\r\n.\r\n"));
    }
}

#[test]
fn phase_unknown_zone_is_exactly_body_and_final_reply() {
    assert!(!Phase::Prepare.is_unknown_on_failure());
    assert!(!Phase::MailFrom.is_unknown_on_failure());
    assert!(!Phase::Rcpt.is_unknown_on_failure());
    assert!(!Phase::DataRequest.is_unknown_on_failure());
    assert!(Phase::Body.is_unknown_on_failure());
    assert!(Phase::FinalReply.is_unknown_on_failure());
}

#[test]
fn classify_definite_table_maps_each_case_to_the_right_provider_error() {
    let reply = |code| SmtpReply { code, text: "x".into() };
    // 535 (and the 53x reauth band) during prepare => reauth class.
    for code in [530u16, 535, 538] {
        let e = classify_definite(Phase::Prepare, Some(&reply(code)));
        assert!(e.requires_reauthentication(), "{code} => reauth");
    }
    // 5xx on the envelope => permanent.
    for (phase, code) in [
        (Phase::MailFrom, 553u16),
        (Phase::MailFrom, 550),
        (Phase::DataRequest, 554),
        (Phase::FinalReply, 554),
    ] {
        let e = classify_definite(phase, Some(&reply(code)));
        assert!(
            matches!(e, ProviderError::PermanentClientRejection(_)),
            "{phase:?} {code} => permanent, got {e:?}"
        );
        assert!(!e.retry_mutation());
    }
    // 4xx => transient/retryable.
    for (phase, code) in [(Phase::Rcpt, 450u16), (Phase::FinalReply, 451)] {
        let e = classify_definite(phase, Some(&reply(code)));
        assert!(
            matches!(e, ProviderError::TransientTransport(_)),
            "{phase:?} {code} => transient, got {e:?}"
        );
        assert!(e.retry_mutation());
    }
    // No reply (transport failure before 354) => transient transport.
    let e = classify_definite(Phase::DataRequest, None);
    assert!(matches!(e, ProviderError::TransientTransport(_)));
}

#[test]
fn credentials_debug_redacts_the_password() {
    let c = SmtpCredentials {
        username: "user@example.com".into(),
        password: "hunter2-very-secret".into(),
    };
    let shown = format!("{c:?}");
    assert!(shown.contains("user@example.com"));
    assert!(!shown.contains("hunter2"), "password must never appear in Debug: {shown}");
    assert!(shown.contains("<redacted>"));
}

#[test]
fn reply_text_sanitiser_strips_controls_and_caps_length() {
    assert_eq!(sanitise_reply_text("ok\r\n done\there"), "ok done here");
    assert_eq!(sanitise_reply_text("plain"), "plain");
    let long = "x".repeat(10_000);
    assert!(sanitise_reply_text(&long).len() <= policy::MAX_SMTP_REPLY_TEXT_BYTES);
}

// The dry-run's SMTP reply reader must handle multi-line continuations and act
// only on the final line's status code. (Moved with the reader from setup.rs.)
#[tokio::test]
async fn smtp_reply_reader_handles_multiline_and_checks_the_final_code() {
    use std::io::Cursor;
    let mut ok = Cursor::new(
        b"250-mail.example.com\r\n250-SIZE 1024\r\n250 AUTH LOGIN\r\n".to_vec(),
    );
    assert!(read_smtp_reply(&mut ok, 250).await.is_ok());
    let mut rejected = Cursor::new(b"535 5.7.8 bad credentials\r\n".to_vec());
    assert!(read_smtp_reply(&mut rejected, 235).await.is_err());
    let mut truncated = Cursor::new(b"25".to_vec());
    assert!(read_smtp_reply(&mut truncated, 250).await.is_err());
}

#[test]
fn too_many_recipients_is_rejected_before_any_io() {
    // The gate is a pure policy check; prove it rejects over-limit counts.
    let over = policy::MAX_SMTP_RECIPIENTS + 1;
    assert!(policy::check_smtp_recipient_count(over).is_err());
    assert!(policy::check_smtp_recipient_count(policy::MAX_SMTP_RECIPIENTS).is_ok());
}

// ---------------------------------------------------------------------------
// Moved setup dry-run tests: behaviour is UNCHANGED after the module move.
// These mirror the originals that lived in setup.rs, exercising the SAME
// `test_smtp` / `probe_smtp_certificate` now re-exported from here.
// ---------------------------------------------------------------------------

mod dry_run {
    use super::*;

    async fn smtp_server(
        mode: SecurityMode,
        reject_password: bool,
        probe_only: bool,
    ) -> (u16, Sha256Fingerprint, tokio::task::JoinHandle<()>) {
        use rustls::pki_types::PrivatePkcs8KeyDer;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
        let pin = Sha256Fingerprint::of_certificate(cert.cert.der());
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            if mode == SecurityMode::StartTls {
                write_all(&mut tcp, "220 submission ready\r\n").await;
                assert_eq!(read_command(&mut tcp).await, "EHLO threestrands.local\r\n");
                write_all(&mut tcp, "250-submission\r\n250 STARTTLS\r\n").await;
                assert_eq!(read_command(&mut tcp).await, "STARTTLS\r\n");
                write_all(&mut tcp, "220 begin TLS\r\n").await;
            }
            let Ok(mut tls) = acceptor.accept(tcp).await else {
                return;
            };
            if mode == SecurityMode::ImplicitTls
                && write_all_res(&mut tls, "220 submission ready\r\n").await.is_err()
            {
                return;
            }
            let first = read_command(&mut tls).await;
            if probe_only {
                assert!(first.is_empty(), "a probe must not send credentials: {first}");
                return;
            }
            assert_eq!(first, "EHLO threestrands.local\r\n");
            write_all(&mut tls, "250-submission\r\n250 AUTH LOGIN\r\n").await;
            assert_eq!(read_command(&mut tls).await, "AUTH LOGIN\r\n");
            write_all(&mut tls, "334 VXNlcm5hbWU6\r\n").await;
            assert_eq!(read_command(&mut tls).await, "dXNlcg==\r\n");
            write_all(&mut tls, "334 UGFzc3dvcmQ6\r\n").await;
            assert_eq!(read_command(&mut tls).await, "c2VjcmV0\r\n");
            if reject_password {
                write_all(&mut tls, "535 bad credentials\r\n").await;
            } else {
                write_all(&mut tls, "235 authenticated\r\n").await;
                assert_eq!(read_command(&mut tls).await, "QUIT\r\n");
            }
        });
        (port, pin, task)
    }

    async fn write_all<S: AsyncWriteExt + Unpin>(s: &mut S, line: &str) {
        s.write_all(line.as_bytes()).await.unwrap();
        s.flush().await.unwrap();
    }
    async fn write_all_res<S: AsyncWriteExt + Unpin>(s: &mut S, line: &str) -> std::io::Result<()> {
        s.write_all(line.as_bytes()).await?;
        s.flush().await
    }

    async fn read_command<S: AsyncReadExt + Unpin>(stream: &mut S) -> String {
        let mut line = Vec::new();
        let mut byte = [0];
        while let Ok(1) = stream.read(&mut byte).await {
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        String::from_utf8(line).unwrap()
    }

    async fn finish(task: tokio::task::JoinHandle<()>) {
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn smtp_authenticates_in_both_tls_modes() {
        for mode in [SecurityMode::ImplicitTls, SecurityMode::StartTls] {
            let (port, pin, server) = smtp_server(mode, false, false).await;
            test_smtp("127.0.0.1", port, mode, Some(pin), "user", "secret")
                .await
                .unwrap();
            finish(server).await;
        }
    }

    #[tokio::test]
    async fn smtp_probes_collect_certificates_without_authenticating() {
        for mode in [SecurityMode::ImplicitTls, SecurityMode::StartTls] {
            let (port, pin, server) = smtp_server(mode, false, true).await;
            let probe = probe_smtp_certificate("127.0.0.1", port, mode).await.unwrap();
            assert_eq!(probe.certificate.sha256_fingerprint, pin.to_hex());
            assert!(!probe.trusted_by_platform);
            finish(server).await;
        }
    }

    #[tokio::test]
    async fn smtp_rejects_wrong_and_missing_pins_before_credentials() {
        for mode in [SecurityMode::ImplicitTls, SecurityMode::StartTls] {
            for pin in [None, Some(Sha256Fingerprint::from_bytes([0xAB; 32]))] {
                let (port, _, server) = smtp_server(mode, false, true).await;
                assert!(test_smtp("127.0.0.1", port, mode, pin, "user", "secret")
                    .await
                    .is_err());
                finish(server).await;
            }
        }
    }

    #[tokio::test]
    async fn smtp_reports_rejected_credentials() {
        let (port, pin, server) = smtp_server(SecurityMode::StartTls, true, false).await;
        let error = test_smtp(
            "127.0.0.1",
            port,
            SecurityMode::StartTls,
            Some(pin),
            "user",
            "secret",
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ProviderError::ReauthenticationRequired(_)));
        finish(server).await;
    }

    #[test]
    fn default_smtp_ports_follow_the_security_mode() {
        assert_eq!(default_smtp_port(SecurityMode::ImplicitTls), 465);
        assert_eq!(default_smtp_port(SecurityMode::StartTls), 587);
    }
}

// ---------------------------------------------------------------------------
// Live harness (GATED; docker only). Mirrors the IMAP live-test convention:
// skipped unless DOVECOT_TEST_FP is set. The SMTP sink is reachable on the
// mapped SMTP port with the SAME self-signed cert as the IMAP side.
//
//   scripts/dovecot-test-server.sh up
//   DOVECOT_TEST_FP="$(scripts/dovecot-test-server.sh fingerprint)" \
//   THREESTRANDS_SMTP_SINK_PORT=11587 THREESTRANDS_SMTP_SINK_IMPLICIT_PORT=11465 \
//     cargo test --lib provider::imap::smtp::tests::live_ -- --nocapture --ignored
//
// NOTE: the worker that wrote these could NOT run docker; see the brief's
// report. They are #[ignore]d so the default `cargo test` never needs docker.
// ---------------------------------------------------------------------------

mod live {
    use super::*;
    use crate::provider::imap::tls::parse_sha256_fingerprint;

    fn sink_endpoint(security: SecurityMode, pin: Sha256Fingerprint) -> Option<SmtpEndpoint> {
        let var = match security {
            SecurityMode::StartTls => "THREESTRANDS_SMTP_SINK_PORT",
            SecurityMode::ImplicitTls => "THREESTRANDS_SMTP_SINK_IMPLICIT_PORT",
        };
        let port: u16 = std::env::var(var)
            .ok()?
            .parse()
            .ok()?;
        Some(SmtpEndpoint {
            host: "127.0.0.1".into(),
            port,
            security,
            pinned: Some(pin),
        })
    }

    fn fp() -> Option<Sha256Fingerprint> {
        let s = std::env::var("DOVECOT_TEST_FP").ok()?;
        parse_sha256_fingerprint(&s).ok()
    }

    #[tokio::test]
    #[ignore = "requires docker SMTP sink (DOVECOT_TEST_FP + THREESTRANDS_SMTP_SINK_PORT)"]
    async fn live_authenticated_send_is_accepted_over_starttls() {
        let Some(fp) = fp() else {
            eprintln!("skipping: DOVECOT_TEST_FP not set");
            return;
        };
        let Some(endpoint) = sink_endpoint(SecurityMode::StartTls, fp) else {
            eprintln!("skipping: THREESTRANDS_SMTP_SINK_PORT not set");
            return;
        };
        let creds = SmtpCredentials {
            username: "test@threestrands.test".into(),
            password: "testpassword".into(),
        };
        let session = SmtpSession::connect(&endpoint, &creds)
            .await
            .expect("live connect+auth should succeed");
        let out = session
            .send(
                "test@threestrands.test",
                &["test@threestrands.test".to_string()],
                b"From: test@threestrands.test\r\nTo: test@threestrands.test\r\n\
                  Subject: live smtp\r\n\r\nhello from the live harness\r\n",
            )
            .await;
        assert!(
            matches!(out, SmtpOutcome::Accepted { .. }),
            "live send should be Accepted, got {out:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires docker SMTP sink (DOVECOT_TEST_FP + THREESTRANDS_SMTP_SINK_PORT)"]
    async fn live_authenticated_send_is_accepted_over_implicit_tls() {
        let Some(fp) = fp() else {
            eprintln!("skipping: DOVECOT_TEST_FP not set");
            return;
        };
        let Some(endpoint) = sink_endpoint(SecurityMode::ImplicitTls, fp) else {
            eprintln!("skipping: THREESTRANDS_SMTP_SINK_PORT not set");
            return;
        };
        let creds = SmtpCredentials {
            username: "test@threestrands.test".into(),
            password: "testpassword".into(),
        };
        let session = SmtpSession::connect(&endpoint, &creds)
            .await
            .expect("live connect+auth should succeed");
        let out = session
            .send(
                "test@threestrands.test",
                &["test@threestrands.test".to_string()],
                b"From: test@threestrands.test\r\nTo: test@threestrands.test\r\n\
                  Subject: live smtp implicit\r\n\r\nhello from the live harness\r\n",
            )
            .await;
        assert!(
            matches!(out, SmtpOutcome::Accepted { .. }),
            "live send should be Accepted, got {out:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires docker SMTP sink"]
    async fn live_wrong_pin_fails_before_auth() {
        if fp().is_none() {
            eprintln!("skipping: DOVECOT_TEST_FP not set");
            return;
        }
        let wrong = Sha256Fingerprint::from_bytes([0xCD; 32]);
        let Some(endpoint) = sink_endpoint(SecurityMode::StartTls, wrong) else {
            eprintln!("skipping: THREESTRANDS_SMTP_SINK_PORT not set");
            return;
        };
        let creds = SmtpCredentials {
            username: "test@threestrands.test".into(),
            password: "testpassword".into(),
        };
        let res = SmtpSession::connect(&endpoint, &creds).await;
        assert!(res.is_err(), "a wrong pin must fail the live handshake");
    }

    #[tokio::test]
    #[ignore = "requires docker SMTP sink"]
    async fn live_bad_password_maps_to_credential_rejection() {
        let Some(fp) = fp() else {
            eprintln!("skipping: DOVECOT_TEST_FP not set");
            return;
        };
        let Some(endpoint) = sink_endpoint(SecurityMode::StartTls, fp) else {
            eprintln!("skipping: THREESTRANDS_SMTP_SINK_PORT not set");
            return;
        };
        let creds = SmtpCredentials {
            username: "test@threestrands.test".into(),
            password: "WRONG".into(),
        };
        let fail = SmtpSession::connect(&endpoint, &creds)
            .await
            .err()
            .expect("a bad password must fail connect");
        assert!(
            fail.error.requires_reauthentication(),
            "a bad password must map to the reauth class, got {:?}",
            fail.error
        );
    }
}
