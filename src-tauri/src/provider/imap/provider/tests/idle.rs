use super::*;
use crate::provider::imap::policy::{IDLE_COMMAND_TIMEOUT, IDLE_RENEW_INTERVAL};
use std::collections::VecDeque;
use tokio::sync::{mpsc, Notify};

#[derive(Clone, Copy)]
enum Behavior {
    Normal,
    Reject,
    StallStart,
    StallDone,
}
enum Action {
    Data(&'static str),
    Disconnect,
}

struct IdleSource {
    command: FakeSource,
    streams: Mutex<VecDeque<DuplexStream>>,
}

#[async_trait]
impl ImapSessionSource for IdleSource {
    async fn open(&self) -> ProviderResult<Box<dyn ImapSession>> {
        self.command.open().await
    }
    async fn open_idle(&self) -> ProviderResult<Option<Box<dyn ImapSession>>> {
        let stream = self
            .streams
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted idle connection");
        let session = async_imap::Client::new(stream)
            .login("user", "password")
            .await
            .map_err(|(error, _)| super::super::super::error::map_imap_error(&error))?;
        Ok(Some(Box::new(
            super::super::super::session::AsyncImapSession::new(session),
        )))
    }
}

struct Wire {
    stream: DuplexStream,
    actions: mpsc::UnboundedSender<Action>,
    entered: mpsc::UnboundedReceiver<()>,
    commands: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

fn wire(capabilities: &'static str, behavior: Behavior) -> Wire {
    let (client, server) = tokio::io::duplex(8192);
    let (actions, mut incoming) = mpsc::unbounded_channel();
    let (ready, entered) = mpsc::unbounded_channel();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let recorded = commands.clone();
    let task = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        reply(&mut server, "* OK ready\r\n").await;
        let mut idle_tag = String::new();
        let mut line = Vec::new();
        loop {
            tokio::select! {
                action = incoming.recv() => match action {
                    Some(Action::Data(text)) => reply(&mut server, text).await,
                    Some(Action::Disconnect) | None => break,
                },
                count = server.read_until(b'\n', &mut line) => {
                    if count.unwrap() == 0 { break; }
                    let command = String::from_utf8(std::mem::take(&mut line)).unwrap();
                    let command = command.trim();
                    if command == "DONE" {
                        recorded.lock().unwrap().push("DONE".into());
                        if !matches!(behavior, Behavior::StallDone) {
                            reply(&mut server, &format!("{idle_tag} OK idle completed\r\n")).await;
                        }
                        continue;
                    }
                    let (tag, rest) = command.split_once(' ').unwrap();
                    let name = rest.split_whitespace().next().unwrap();
                    recorded.lock().unwrap().push(if name == "LOGIN" { "LOGIN".into() } else { rest.into() });
                    match name {
                        "LOGIN" => reply(&mut server, &format!("{tag} OK login\r\n")).await,
                        "CAPABILITY" => reply(&mut server, &format!("* CAPABILITY IMAP4rev1 {}\r\n{tag} OK capabilities\r\n", capabilities)).await,
                        "EXAMINE" => reply(&mut server, &format!("* 0 EXISTS\r\n* OK [UIDVALIDITY 1] valid\r\n{tag} OK [READ-ONLY] examined\r\n")).await,
                        "IDLE" => {
                            idle_tag = tag.into();
                            match behavior {
                                Behavior::Reject => reply(&mut server, &format!("{tag} BAD IDLE disabled\r\n")).await,
                                Behavior::StallStart => {},
                                _ => reply(&mut server, "+ idling\r\n").await,
                            }
                            let _ = ready.send(());
                        },
                        "LOGOUT" => {
                            reply(&mut server, &format!("* BYE closing\r\n{tag} OK logout\r\n")).await;
                            break;
                        },
                        _ => panic!("unexpected watch command {command}"),
                    }
                }
            }
        }
    });
    Wire {
        stream: client,
        actions,
        entered,
        commands,
        task,
    }
}

fn watched_provider(
    mailbox: Arc<Mutex<FakeMailbox>>,
    streams: Vec<DuplexStream>,
) -> Arc<ImapProvider> {
    let (mut provider, _) = provider_with(mailbox.clone());
    provider.source = Arc::new(IdleSource {
        command: FakeSource { mailbox },
        streams: Mutex::new(streams.into()),
    });
    Arc::new(provider)
}

fn start_watch(
    provider: Arc<ImapProvider>,
) -> (Arc<Notify>, tokio::task::JoinHandle<ProviderResult<()>>) {
    let changed = Arc::new(Notify::new());
    let callback = changed.clone();
    let task = tokio::spawn(async move {
        provider
            .watch_inbox(Arc::new(move || callback.notify_one()))
            .await
    });
    (changed, task)
}

#[tokio::test]
async fn idle_notifications_reach_normal_sync_for_new_mail_flags_and_expunges() {
    let mut mailbox = FakeMailbox::new(100);
    mailbox.add(&[], &message("<one@idle>", "One", ""));
    let mailbox = Arc::new(Mutex::new(mailbox));
    let Wire {
        stream,
        actions,
        mut entered,
        commands,
        task: server,
    } = wire("IDLE", Behavior::Normal);
    let provider = watched_provider(mailbox.clone(), vec![stream]);
    let db = provider.store.database().clone();
    crate::sync::sync_with(&db, "me@example.com", provider.as_ref())
        .await
        .unwrap();
    let (changed, task) = start_watch(provider.clone());
    entered.recv().await.unwrap();
    changed.notified().await;
    assert_eq!(
        &commands.lock().unwrap()[..],
        ["LOGIN", "CAPABILITY", "EXAMINE \"INBOX\"", "IDLE"]
    );

    mailbox
        .lock()
        .unwrap()
        .add(&[], &message("<two@idle>", "Two", ""));
    actions.send(Action::Data("* 2 EXISTS\r\n")).unwrap();
    changed.notified().await;
    assert!(
        crate::sync::sync_with(&db, "me@example.com", provider.as_ref())
            .await
            .unwrap()
    );
    assert_eq!(provider.list_inbox(None).await.unwrap().thread_ids.len(), 2);
    assert!(mailbox
        .lock()
        .unwrap()
        .inbox()
        .messages
        .iter()
        .all(|message| message.flags.is_empty()));

    entered.recv().await.unwrap();
    changed.notified().await; // Drain catch-up after re-entering IDLE.
    {
        let mut mailbox = mailbox.lock().unwrap();
        mailbox.inbox().messages[0].flags = vec!["\\Seen".into()];
        mailbox.inbox().messages.retain(|message| message.uid == 1);
    }
    actions
        .send(Action::Data(
            "* 1 FETCH (FLAGS (\\Seen))\r\n* 2 EXPUNGE\r\n",
        ))
        .unwrap();
    changed.notified().await;
    assert!(
        crate::sync::sync_with(&db, "me@example.com", provider.as_ref())
            .await
            .unwrap()
    );
    let threads = provider.list_inbox(None).await.unwrap().thread_ids;
    assert_eq!(threads.len(), 1);
    assert!(!provider.fetch_thread(&threads[0]).await.unwrap()[0]
        .label_ids
        .contains(&"UNREAD".into()));
    task.abort();
    let _ = task.await;
    tokio::time::timeout(std::time::Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn servers_without_idle_release_the_watch_connection_and_keep_polling() {
    let Wire {
        stream,
        actions: _actions,
        commands,
        task: server,
        ..
    } = wire("XIDLE IDLE-NO", Behavior::Normal);
    let provider = watched_provider(Arc::new(Mutex::new(FakeMailbox::new(1))), vec![stream]);
    let (_changed, task) = start_watch(provider.clone());
    server.await.unwrap();
    assert_eq!(
        &commands.lock().unwrap()[..],
        ["LOGIN", "CAPABILITY", "LOGOUT"]
    );
    provider.baseline_cursor().await.unwrap();
    assert!(!task.is_finished());
    task.abort();
    let _ = task.await;
}

#[tokio::test(start_paused = true)]
async fn idle_renews_at_twenty_five_minutes_despite_keepalives() {
    let Wire {
        stream,
        actions,
        mut entered,
        commands,
        task: server,
    } = wire("IDLE", Behavior::Normal);
    let provider = watched_provider(Arc::new(Mutex::new(FakeMailbox::new(1))), vec![stream]);
    let (_changed, task) = start_watch(provider);
    entered.recv().await.unwrap();
    for _ in 0..24 {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        actions.send(Action::Data("* OK still here\r\n")).unwrap();
        tokio::task::yield_now().await;
    }
    assert_eq!(
        commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| *command == "IDLE")
            .count(),
        1
    );
    tokio::time::sleep(IDLE_RENEW_INTERVAL - std::time::Duration::from_secs(24 * 60)).await;
    entered.recv().await.unwrap();
    assert_eq!(&commands.lock().unwrap()[4..], ["DONE", "IDLE"]);
    task.abort();
    let _ = task.await;
    server.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn idle_rejection_stalled_handshakes_and_disconnects_are_errors_with_polling_fallback() {
    for behavior in [
        Behavior::Reject,
        Behavior::StallStart,
        Behavior::StallDone,
        Behavior::Normal,
    ] {
        let Wire {
            stream,
            actions,
            mut entered,
            task: server,
            ..
        } = wire("IDLE", behavior);
        let provider = watched_provider(Arc::new(Mutex::new(FakeMailbox::new(1))), vec![stream]);
        let (_changed, task) = start_watch(provider.clone());
        entered.recv().await.unwrap();
        match behavior {
            Behavior::StallStart => tokio::time::sleep(IDLE_COMMAND_TIMEOUT).await,
            Behavior::StallDone => {
                actions.send(Action::Data("* 1 EXISTS\r\n")).unwrap();
            }
            Behavior::Normal => actions.send(Action::Disconnect).unwrap(),
            Behavior::Reject => {}
        }
        let error = task.await.unwrap().unwrap_err();
        assert!(!error.requires_reauthentication(), "{error:?}");
        provider.baseline_cursor().await.unwrap();
        server.await.unwrap();
    }
}
