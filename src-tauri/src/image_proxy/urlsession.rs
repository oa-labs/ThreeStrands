//! A second attempt for remote images whose host refused the proxy's own
//! reqwest client, made through the system's URLSession.
//!
//! Some CDN bot filters (Cloudflare's managed challenge, for one) judge the
//! TLS handshake rather than the request headers, and answer a non-browser
//! handshake with a 403 challenge page however browser-like the headers are.
//! URLSession shares Safari's network stack, so those hosts serve it the
//! image. It is only ever a fallback: [`super::download`] calls it after a
//! 403 that isn't an image, never for an ordinary load.
//!
//! URLSession resolves hostnames itself, so the guarded resolver that
//! protects the reqwest path can't be attached here. Instead [`PeerPolicy`]
//! is applied three ways:
//! - every request URL, the first and each redirect target, is checked
//!   (including resolving it) before URLSession is allowed to load it;
//! - every address URLSession reports having connected to is checked after
//!   the fact, and the whole response is discarded if any was refused;
//! - the session is ephemeral, with no cookies, cache, credential storage,
//!   or proxies, and it never answers a login or client-certificate request.
//!
//! What remains is a host that resolves to a public address when checked and
//! a private one a moment later when URLSession connects (DNS rebinding):
//! that request is still sent, though its response is never used. The
//! reqwest path doesn't have this gap; this path only runs after a public
//! host has already turned the proxy away.

use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

use block2::DynBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_foundation::{
    ns_string, NSData, NSDictionary, NSError, NSHTTPURLResponse, NSMutableURLRequest,
    NSOperationQueue, NSString, NSURLAuthenticationChallenge, NSURLAuthenticationMethodServerTrust,
    NSURLCredential, NSURLRequest, NSURLRequestCachePolicy, NSURLSession,
    NSURLSessionAuthChallengeDisposition, NSURLSessionConfiguration, NSURLSessionDataDelegate,
    NSURLSessionDataTask, NSURLSessionDelegate, NSURLSessionTask, NSURLSessionTaskDelegate,
    NSURLSessionTaskMetrics, NSURL,
};
use tokio::sync::oneshot;
use url::Url;

/// Matches the reqwest client's `Policy::limited(5)`.
const MAX_REDIRECTS: usize = 5;

/// Which hosts the fallback may reach. Production passes the proxy's
/// public-host rules; tests swap in rules that admit a loopback listener.
#[derive(Clone, Copy)]
pub(super) struct PeerPolicy {
    /// Whether a request URL (the first one or a redirect target) may be
    /// loaded. Called off the async runtime, so it may block on DNS.
    pub(super) check_target: fn(&Url) -> Result<(), String>,
    /// Whether an address URLSession actually connected to is acceptable.
    pub(super) allows_peer: fn(IpAddr) -> bool,
}

pub(super) struct PlatformResponse {
    pub(super) status: u16,
    /// The raw `Content-Type` header, or empty when the host sent none.
    pub(super) content_type: String,
    pub(super) bytes: Vec<u8>,
}

type Outcome = Result<PlatformResponse, String>;

struct Ivars {
    policy: PeerPolicy,
    max_bytes: usize,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    bytes: Vec<u8>,
    /// The first reason this load must fail, kept even after URLSession
    /// reports its own (less specific) cancellation error.
    failure: Option<String>,
    redirects: usize,
    peers_confirmed: bool,
    sender: Option<oneshot::Sender<Outcome>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ThreeStrandsImageFetchDelegate"]
    #[ivars = Ivars]
    struct FetchDelegate;

    unsafe impl NSObjectProtocol for FetchDelegate {}

    unsafe impl NSURLSessionDelegate for FetchDelegate {}

    unsafe impl NSURLSessionTaskDelegate for FetchDelegate {
        #[unsafe(method(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:))]
        fn will_perform_redirection(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionTask,
            _response: &NSHTTPURLResponse,
            request: &NSURLRequest,
            completion: &DynBlock<dyn Fn(*mut NSURLRequest)>,
        ) {
            match self.check_redirect(request) {
                Ok(()) => completion.call((request as *const NSURLRequest as *mut NSURLRequest,)),
                Err(error) => {
                    self.fail(error);
                    // A null request stops here and delivers the redirect
                    // response itself, which then fails on `failure`.
                    completion.call((std::ptr::null_mut(),));
                }
            }
        }

        #[unsafe(method(URLSession:task:didReceiveChallenge:completionHandler:))]
        fn did_receive_challenge(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionTask,
            challenge: &NSURLAuthenticationChallenge,
            completion: &DynBlock<
                dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential),
            >,
        ) {
            // Certificates are validated as usual. A login, proxy login, or
            // client-certificate request is refused, so an image host can
            // never prompt for or receive anything that identifies the reader.
            let method = challenge.protectionSpace().authenticationMethod();
            let disposition = if &*method == unsafe { NSURLAuthenticationMethodServerTrust } {
                NSURLSessionAuthChallengeDisposition::PerformDefaultHandling
            } else {
                NSURLSessionAuthChallengeDisposition::CancelAuthenticationChallenge
            };
            completion.call((disposition, std::ptr::null_mut()));
        }

        #[unsafe(method(URLSession:task:didFinishCollectingMetrics:))]
        fn did_finish_collecting_metrics(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionTask,
            metrics: &NSURLSessionTaskMetrics,
        ) {
            let addresses: Vec<Option<String>> = metrics
                .transactionMetrics()
                .to_vec()
                .iter()
                .map(|transaction| {
                    transaction
                        .remoteAddress()
                        .map(|address| address.to_string())
                })
                .collect();
            match confirm_peers(&addresses, self.ivars().policy.allows_peer) {
                Ok(()) => self.state().peers_confirmed = true,
                Err(error) => self.fail(error),
            }
        }

        #[unsafe(method(URLSession:task:didCompleteWithError:))]
        fn did_complete(
            &self,
            _session: &NSURLSession,
            task: &NSURLSessionTask,
            error: Option<&NSError>,
        ) {
            let mut state = self.state();
            let Some(sender) = state.sender.take() else {
                return;
            };
            let outcome = if let Some(failure) = state.failure.take() {
                Err(failure)
            } else if let Some(error) = error {
                Err(format!(
                    "Image request failed: {}",
                    error.localizedDescription()
                ))
            } else if !state.peers_confirmed {
                // Fail closed if URLSession never reported where it connected.
                Err("Image host's address could not be confirmed".to_string())
            } else {
                http_response(task, std::mem::take(&mut state.bytes))
            };
            let _ = sender.send(outcome);
        }
    }

    unsafe impl NSURLSessionDataDelegate for FetchDelegate {
        #[unsafe(method(URLSession:dataTask:didReceiveData:))]
        fn did_receive_data(
            &self,
            _session: &NSURLSession,
            task: &NSURLSessionDataTask,
            data: &NSData,
        ) {
            let mut state = self.state();
            if state.failure.is_some() {
                return;
            }
            // URLSession hands over decoded bytes, so like the reqwest path
            // the limit applies to the image itself.
            if data.len() > self.ivars().max_bytes.saturating_sub(state.bytes.len()) {
                state.failure = Some("Image exceeds the maximum allowed size".to_string());
                drop(state);
                task.cancel();
                return;
            }
            state.bytes.extend_from_slice(&data.to_vec());
        }
    }
);

impl FetchDelegate {
    fn new(
        policy: PeerPolicy,
        max_bytes: usize,
        sender: oneshot::Sender<Outcome>,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars {
            policy,
            max_bytes,
            state: Mutex::new(State {
                sender: Some(sender),
                ..State::default()
            }),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.ivars()
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn fail(&self, error: String) {
        self.state().failure.get_or_insert(error);
    }

    fn check_redirect(&self, request: &NSURLRequest) -> Result<(), String> {
        {
            let mut state = self.state();
            state.redirects += 1;
            if state.redirects > MAX_REDIRECTS {
                return Err("Image request redirected too many times".to_string());
            }
        }
        let target = request
            .URL()
            .and_then(|url| url.absoluteString())
            .ok_or_else(|| "Image redirect has no URL".to_string())?;
        let target = Url::parse(&target.to_string())
            .map_err(|_| "Invalid image redirect URL".to_string())?;
        (self.ivars().policy.check_target)(&target)
    }
}

fn http_response(task: &NSURLSessionTask, bytes: Vec<u8>) -> Outcome {
    let response = task
        .response()
        .and_then(|response| response.downcast::<NSHTTPURLResponse>().ok())
        .ok_or_else(|| "Image request returned no HTTP response".to_string())?;
    let status = u16::try_from(response.statusCode())
        .map_err(|_| "Image request returned an invalid status".to_string())?;
    let content_type = response
        .valueForHTTPHeaderField(ns_string!("Content-Type"))
        .map(|value| value.to_string())
        .unwrap_or_default();
    Ok(PlatformResponse {
        status,
        content_type,
        bytes,
    })
}

/// URLSession reports addresses as text; IPv6 link-local ones may carry a
/// `%interface` scope suffix, which isn't part of the address.
fn parse_peer(address: &str) -> Option<IpAddr> {
    address.split('%').next()?.parse().ok()
}

/// Every transaction (one per redirect hop) must name an address, and every
/// address must pass the policy; anything unreadable counts as refused.
fn confirm_peers(
    addresses: &[Option<String>],
    allows_peer: fn(IpAddr) -> bool,
) -> Result<(), String> {
    if addresses.is_empty() {
        return Err("Image host's address could not be confirmed".to_string());
    }
    for address in addresses {
        match address.as_deref().and_then(parse_peer) {
            Some(peer) if allows_peer(peer) => {}
            Some(_) => return Err("Image request reached a local or private host".to_string()),
            None => return Err("Image host's address could not be confirmed".to_string()),
        }
    }
    Ok(())
}

/// Cancels the URLSession task if the caller stops waiting (a timeout, or the
/// reader navigating away); cancelling a finished task does nothing.
struct CancelOnDrop(Retained<NSURLSessionDataTask>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(super) async fn fetch(
    url: &Url,
    policy: PeerPolicy,
    user_agent: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
    max_bytes: usize,
) -> Outcome {
    let target = url.clone();
    tokio::task::spawn_blocking(move || (policy.check_target)(&target))
        .await
        .map_err(|_| "Image request check failed".to_string())??;

    let (sender, receiver) = oneshot::channel();
    let _task = CancelOnDrop(start(
        url, policy, user_agent, headers, timeout, max_bytes, sender,
    )?);
    // URLSession enforces `timeout` itself; this only guards against a
    // completion that never arrives.
    match tokio::time::timeout(timeout + Duration::from_secs(1), receiver).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_)) => Err("Image request ended without a result".to_string()),
        Err(_) => Err("Image request timed out".to_string()),
    }
}

fn start(
    url: &Url,
    policy: PeerPolicy,
    user_agent: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
    max_bytes: usize,
    sender: oneshot::Sender<Outcome>,
) -> Result<Retained<NSURLSessionDataTask>, String> {
    let configuration = NSURLSessionConfiguration::ephemeralSessionConfiguration();
    configuration.setHTTPCookieStorage(None);
    configuration.setHTTPShouldSetCookies(false);
    configuration.setURLCache(None);
    configuration.setURLCredentialStorage(None);
    configuration.setRequestCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData);
    configuration.setTimeoutIntervalForRequest(timeout.as_secs_f64());
    configuration.setTimeoutIntervalForResource(timeout.as_secs_f64());
    // An empty proxy dictionary turns system proxies off, as on the reqwest
    // path, so the address metrics report is the image host's own.
    let no_proxies = NSDictionary::<AnyObject, AnyObject>::new();
    unsafe { configuration.setConnectionProxyDictionary(Some(&no_proxies)) };

    let ns_url = NSURL::URLWithString(&NSString::from_str(url.as_str()))
        .ok_or_else(|| "Invalid image URL".to_string())?;
    let request = NSMutableURLRequest::requestWithURL(&ns_url);
    request.setHTTPShouldHandleCookies(false);
    request.setValue_forHTTPHeaderField(
        Some(&NSString::from_str(user_agent)),
        ns_string!("User-Agent"),
    );
    // Accept-Encoding is left to URLSession, which then decodes the body.
    for (name, value) in headers {
        request.setValue_forHTTPHeaderField(
            Some(&NSString::from_str(value)),
            &NSString::from_str(name),
        );
    }

    let delegate = FetchDelegate::new(policy, max_bytes, sender);
    let queue = NSOperationQueue::new();
    queue.setMaxConcurrentOperationCount(1);
    let session = unsafe {
        NSURLSession::sessionWithConfiguration_delegate_delegateQueue(
            &configuration,
            Some(ProtocolObject::from_ref(&*delegate)),
            Some(&queue),
        )
    };
    let task = session.dataTaskWithRequest(&request);
    task.resume();
    // A session holds its delegate until invalidated; this releases both
    // once the one task finishes.
    session.finishTasksAndInvalidate();
    Ok(task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Admits the loopback listeners below, except any path starting with
    /// `/forbidden`, which stands in for a private redirect target.
    const LOOPBACK_POLICY: PeerPolicy = PeerPolicy {
        check_target: |url| {
            if url.host_str() == Some("127.0.0.1") && !url.path().starts_with("/forbidden") {
                Ok(())
            } else {
                Err("Image URL points to a local or private host".to_string())
            }
        },
        allows_peer: |peer| peer.is_loopback(),
    };

    const TIMEOUT: Duration = Duration::from_secs(5);

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8(request).unwrap().to_ascii_lowercase()
    }

    /// Answers up to `limit` connections with `response(n)` and returns the
    /// requests received, waiting at most a moment for each connection.
    fn serve(
        limit: usize,
        response: impl Fn(usize) -> Vec<u8> + Send + 'static,
    ) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while requests.len() < limit && std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        requests.push(read_request(&mut stream));
                        stream.write_all(&response(requests.len())).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
            requests
        });
        (port, server)
    }

    fn ok_response(content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn redirect_response(location: &str) -> Vec<u8> {
        format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .into_bytes()
    }

    async fn fetch_with(policy: PeerPolicy, url: &str, max_bytes: usize) -> Outcome {
        fetch(
            &Url::parse(url).unwrap(),
            policy,
            "Mozilla/5.0 Test",
            &[("accept", "image/png"), ("sec-fetch-dest", "image")],
            TIMEOUT,
            max_bytes,
        )
        .await
    }

    #[tokio::test]
    async fn returns_the_hosts_status_type_and_body_with_the_fixed_headers() {
        let (port, server) = serve(1, |_| ok_response("image/png; charset=binary", b"PNGDATA"));
        let response = fetch_with(
            LOOPBACK_POLICY,
            &format!("http://127.0.0.1:{port}/a.png"),
            1024,
        )
        .await
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "image/png; charset=binary");
        assert_eq!(response.bytes, b"PNGDATA");

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(
            request.contains("\r\nuser-agent: mozilla/5.0 test\r\n"),
            "{request}"
        );
        assert!(request.contains("\r\naccept: image/png\r\n"), "{request}");
        assert!(
            request.contains("\r\nsec-fetch-dest: image\r\n"),
            "{request}"
        );
        assert!(!request.contains("\r\ncookie:"), "{request}");
        assert!(!request.contains("\r\nauthorization:"), "{request}");
    }

    #[tokio::test]
    async fn never_connects_to_a_refused_starting_url() {
        let (port, server) = serve(1, |_| ok_response("image/png", b"PNGDATA"));
        let error = fetch_with(
            LOOPBACK_POLICY,
            &format!("http://127.0.0.1:{port}/forbidden.png"),
            1024,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error, "Image URL points to a local or private host");
        assert!(
            server.join().unwrap().is_empty(),
            "the refused host must not be contacted"
        );
    }

    #[tokio::test]
    async fn refuses_a_redirect_to_a_disallowed_target_without_following_it() {
        let (target_port, target) = serve(1, |_| ok_response("image/png", b"PNGDATA"));
        let (port, origin) = serve(1, move |_| {
            redirect_response(&format!("http://127.0.0.1:{target_port}/forbidden.png"))
        });
        let error = fetch_with(
            LOOPBACK_POLICY,
            &format!("http://127.0.0.1:{port}/a.png"),
            1024,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error, "Image URL points to a local or private host");
        assert_eq!(origin.join().unwrap().len(), 1);
        assert!(
            target.join().unwrap().is_empty(),
            "the redirect target must not be contacted"
        );
    }

    #[tokio::test]
    async fn follows_allowed_redirects_up_to_the_limit() {
        let (port, server) = serve(MAX_REDIRECTS + 2, |_| redirect_response("/again"));
        let error = fetch_with(
            LOOPBACK_POLICY,
            &format!("http://127.0.0.1:{port}/a.png"),
            1024,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error, "Image request redirected too many times");
        assert_eq!(server.join().unwrap().len(), MAX_REDIRECTS + 1);
    }

    #[tokio::test]
    async fn discards_a_response_from_a_refused_peer_address() {
        let refuse_every_peer = PeerPolicy {
            allows_peer: |_| false,
            ..LOOPBACK_POLICY
        };
        let (port, server) = serve(1, |_| ok_response("image/png", b"PNGDATA"));
        let error = fetch_with(
            refuse_every_peer,
            &format!("http://127.0.0.1:{port}/a.png"),
            1024,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error, "Image request reached a local or private host");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn admits_the_byte_limit_and_refuses_one_byte_more() {
        let body = vec![b'x'; 64];
        let served = body.clone();
        let (port, server) = serve(1, move |_| ok_response("image/png", &served));
        let response = fetch_with(
            LOOPBACK_POLICY,
            &format!("http://127.0.0.1:{port}/a.png"),
            64,
        )
        .await
        .unwrap();
        assert_eq!(response.bytes, body);
        server.join().unwrap();

        let served = vec![b'x'; 65];
        let (port, server) = serve(1, move |_| ok_response("image/png", &served));
        let error = fetch_with(
            LOOPBACK_POLICY,
            &format!("http://127.0.0.1:{port}/a.png"),
            64,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error, "Image exceeds the maximum allowed size");
        server.join().unwrap();
    }

    #[test]
    fn reads_reported_peer_addresses() {
        assert_eq!(
            parse_peer("93.184.216.34"),
            Some("93.184.216.34".parse().unwrap())
        );
        assert_eq!(
            parse_peer("2606:4700::1111"),
            Some("2606:4700::1111".parse().unwrap())
        );
        assert_eq!(parse_peer("fe80::1%en0"), Some("fe80::1".parse().unwrap()));
        assert_eq!(parse_peer("not an address"), None);
    }

    #[test]
    fn confirms_peers_only_when_every_hop_is_known_and_allowed() {
        let public = |peer: IpAddr| !crate::net_safety::is_private_ip(peer);
        let known = |values: &[&str]| {
            values
                .iter()
                .map(|value| Some(value.to_string()))
                .collect::<Vec<_>>()
        };
        assert!(confirm_peers(&known(&["93.184.216.34", "1.1.1.1"]), public).is_ok());
        assert!(confirm_peers(&known(&["93.184.216.34", "192.168.1.1"]), public).is_err());
        assert!(confirm_peers(&known(&["fe80::1%en0"]), public).is_err());
        assert!(confirm_peers(&[Some("93.184.216.34".into()), None], public).is_err());
        assert!(confirm_peers(&[], public).is_err());
    }
}
