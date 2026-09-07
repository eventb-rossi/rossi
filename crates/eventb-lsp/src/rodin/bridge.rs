//! Reach into a *running* Rodin through the bridge plug-in.
//!
//! Everything else in this module tree talks to Rodin through the file
//! system: the build writes a project, Eclipse's seeded auto-refresh
//! eventually notices, and what Rodin saves comes back through the watcher in
//! [`super::sync`]. That works with an unmodified Rodin and stays the
//! fallback for one.
//!
//! What it cannot do is anything that needs the live instance. The most
//! visible case is registering a project while Rodin holds the workspace: the
//! headless Ant runner in [`super::launch`] would need the same Eclipse
//! instance area, so it can never run then, and the user is told to reach for
//! `File > Import` instead.
//!
//! The bridge plug-in (shipped in the eventb-rossi Rodin bundle) opens a
//! loopback socket inside Rodin's JVM and publishes it as
//! `<workspace>/.rossi-bridge/port`. This module is the client for it:
//! read the descriptor, connect, handshake, ask. Every failure (no file, a
//! stale file, a protocol mismatch, a refused connection, a plug-in that does
//! not offer the method) is reported as an error, so the caller falls back to
//! the file-mediated path unchanged.
//!
//! Connections are made per operation rather than held open. Nothing here
//! needs to *receive* an unsolicited message yet, and a fresh connection is
//! also the only honest answer to "is a bridge really there?": a Rodin that
//! was killed leaves its descriptor behind, so only the handshake proves
//! anything.

use std::collections::{BTreeSet, HashMap};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, oneshot};

/// Directory holding the bridge's descriptor, inside the Rodin workspace.
const BRIDGE_DIR: &str = ".rossi-bridge";

/// The descriptor's file name inside [`BRIDGE_DIR`].
const PORT_FILE: &str = "port";

/// The protocol revision this client speaks. A plug-in announcing a different
/// one is left alone; new methods are negotiated through `caps` instead, so
/// this only moves for a change no capability can describe.
const PROTOCOL: i64 = 1;

/// How long connecting and handshaking may take. The peer is on loopback, so
/// this expires only on a stale descriptor, and the lens flow waits on it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long one request may take. `project/register` refreshes a whole
/// project tree, so this is generous; it exists so that a wedged Rodin cannot
/// wedge the caller.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest framed message this client will read. The bridge's answers run
/// to a few hundred bytes; the cap is what keeps a stale descriptor whose port
/// some unrelated process now owns from making this server allocate whatever
/// that process cares to claim.
const MAX_MESSAGE_BYTES: usize = 1 << 20;

/// The most header lines one message may carry, so a peer that never sends the
/// blank line cannot spin the reader forever.
const MAX_HEADER_LINES: usize = 32;

/// Reported when the reader has stopped: the peer is gone.
const CLOSED: &str = "the Rodin bridge closed the connection";

/// Register a project directory with the running workspace.
pub const REGISTER: &str = "project/register";

/// Bring Rodin forward and select the project.
const REVEAL: &str = "project/reveal";

/// The descriptor the plug-in publishes. Only what a client needs to reach it
/// is read; everything else there, the protocol revision included, is for a
/// human reading the file, since `bridge/hello` is what actually decides.
/// Unknown members are ignored, so a newer plug-in stays readable.
#[derive(Deserialize)]
struct Descriptor {
    port: u16,
    /// The secret the plug-in will only answer a client that presents. Being
    /// able to read it out of the workspace is the proof; a plug-in old
    /// enough not to publish one is talked to without it.
    #[serde(default)]
    token: Option<String>,
}

/// What `bridge/hello` answers, and the authority on what may be asked for.
#[derive(Deserialize)]
struct Hello {
    #[serde(default)]
    protocol: i64,
    #[serde(default)]
    rodin: String,
    #[serde(default)]
    caps: BTreeSet<String>,
}

/// A live connection to the bridge plug-in of one Rodin.
pub struct Bridge {
    transport: Transport,
    caps: BTreeSet<String>,
    rodin: String,
}

impl Bridge {
    /// Connect to the bridge published for `workspace_dir` and handshake.
    pub async fn connect(workspace_dir: &Path) -> Result<Self, String> {
        let descriptor = read_descriptor(workspace_dir)?;
        let transport = Transport::connect(descriptor.port).await?;
        // The handshake gets the shorter timeout: a stale descriptor whose
        // port someone else now owns is exactly the case that accepts the
        // connection and then never answers.
        let hello = transport
            .request_within(
                "bridge/hello",
                json!({"client": "rossi-lsp", "token": descriptor.token}),
                CONNECT_TIMEOUT,
            )
            .await?;
        let hello: Hello = serde_json::from_value(hello)
            .map_err(|e| format!("bridge/hello answered something unreadable: {e}"))?;
        check_protocol(hello.protocol)?;
        Ok(Self {
            transport,
            caps: hello.caps,
            rodin: hello.rodin,
        })
    }

    /// Whether the plug-in offers a method, such as [`REGISTER`].
    pub fn supports(&self, method: &str) -> bool {
        self.caps.contains(method)
    }

    /// The Rodin platform version the plug-in reported.
    pub fn rodin_version(&self) -> &str {
        &self.rodin
    }

    /// Create and open the project at `project_dir` in the running workspace,
    /// and refresh it. Doing this twice is harmless.
    ///
    /// Answers the name the workspace registered it under, when the plug-in
    /// reports one: that name comes from the `.project` descriptor, not from
    /// the directory, so it need not be the one the caller derived.
    pub async fn register_project(&self, project_dir: &Path) -> Result<Option<String>, String> {
        let path = project_dir
            .to_str()
            .ok_or_else(|| format!("{} is not valid UTF-8", project_dir.display()))?;
        let answer = self
            .transport
            .request(REGISTER, json!({ "path": path }))
            .await?;
        Ok(answer
            .get("project")
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// Bring Rodin forward and select the project in the Event-B Explorer.
    pub async fn reveal_project(&self, project_name: &str) -> Result<(), String> {
        self.transport
            .request(REVEAL, json!({ "project": project_name }))
            .await
            .map(|_| ())
    }
}

/// Where the plug-in publishes its descriptor for a workspace.
fn descriptor_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(BRIDGE_DIR).join(PORT_FILE)
}

/// Read the published descriptor. A blocking read of a few hundred bytes, on
/// the same async path where `lock::workspace_lock_state` already probes the
/// workspace's lock file the same way; `tokio::fs` would mean a new feature on
/// the workspace's tokio for one of the two.
fn read_descriptor(workspace_dir: &Path) -> Result<Descriptor, String> {
    let path = descriptor_path(workspace_dir);
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("no Rodin bridge published at {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| format!("{} is not a bridge descriptor: {e}", path.display()))
}

fn check_protocol(protocol: i64) -> Result<(), String> {
    if protocol == PROTOCOL {
        return Ok(());
    }
    Err(format!(
        "the Rodin bridge speaks protocol {protocol}, this server speaks {PROTOCOL}"
    ))
}

/// Requests in flight, keyed by their JSON-RPC id. `None` once the reader has
/// stopped: the peer is gone, so a request made after that must fail at once
/// rather than wait out [`REQUEST_TIMEOUT`] for an answer nobody will send.
type Pending = Arc<Mutex<Option<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>>;

/// The framed JSON-RPC connection. A reader task routes answers back to the
/// caller that is waiting for them; writes go out under a lock, since they
/// are small and rare.
struct Transport {
    writer: Mutex<OwnedWriteHalf>,
    pending: Pending,
    next_id: AtomicI64,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl Transport {
    async fn connect(port: u16) -> Result<Self, String> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
            .await
            .map_err(|_| format!("connecting to the Rodin bridge on port {port} timed out"))?
            .map_err(|e| format!("cannot reach the Rodin bridge on port {port}: {e}"))?;
        let _ = stream.set_nodelay(true);
        let (read_half, write_half) = stream.into_split();
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let reader = tokio::spawn(read_loop(read_half, Arc::clone(&pending)));
        Ok(Self {
            writer: Mutex::new(write_half),
            pending,
            next_id: AtomicI64::new(1),
            reader,
        })
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.request_within(method, params, REQUEST_TIMEOUT).await
    }

    /// As [`Self::request`], but bounded by `timeout` rather than
    /// [`REQUEST_TIMEOUT`].
    async fn request_within(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        {
            // Enrolling under the same lock the reader clears on its way out
            // is what makes "the peer went away" and "a request started"
            // order against each other at all.
            let mut pending = self.pending.lock().await;
            let Some(pending) = pending.as_mut() else {
                return Err(CLOSED.to_string());
            };
            pending.insert(id, sender);
        }

        let payload =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        if let Err(message) = self.write(&payload).await {
            self.forget(id).await;
            return Err(message);
        }

        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(format!("the Rodin bridge dropped {method}")),
            Err(_) => {
                self.forget(id).await;
                Err(format!("the Rodin bridge did not answer {method} in time"))
            }
        }
    }

    /// Stop waiting for `id`'s answer.
    async fn forget(&self, id: i64) {
        if let Some(pending) = self.pending.lock().await.as_mut() {
            pending.remove(&id);
        }
    }

    async fn write(&self, payload: &str) -> Result<(), String> {
        // `len` is the byte length, which is what the header must carry.
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        let mut writer = self.writer.lock().await;
        writer
            .write_all(header.as_bytes())
            .await
            .and(writer.write_all(payload.as_bytes()).await)
            .and(writer.flush().await)
            .map_err(|e| format!("cannot write to the Rodin bridge: {e}"))
    }
}

/// Route answers to their waiting callers until the connection ends.
async fn read_loop(read_half: OwnedReadHalf, pending: Pending) {
    let mut reader = BufReader::new(read_half);
    while let Some(mut message) = read_message(&mut reader).await {
        // A message without a usable id is one of two things. It may be a
        // notification: `bridge/shutdown` is the only one, and the connection
        // closing right behind it is what the client acts on. Or it may be an
        // error the peer could not attribute to a request, which JSON-RPC
        // answers with a null id; that kind says it could not read what we
        // sent, so nothing in flight is going to be answered either.
        let Some(id) = message.get("id").and_then(Value::as_i64) else {
            if let Some(error) = message.get("error") {
                let reason = error_message(error);
                let Some(in_flight) = pending.lock().await.as_mut().map(std::mem::take) else {
                    break;
                };
                for (_, sender) in in_flight {
                    let _ = sender.send(Err(reason.clone()));
                }
            }
            continue;
        };
        let Some(sender) = pending.lock().await.as_mut().and_then(|p| p.remove(&id)) else {
            continue;
        };
        let _ = sender.send(match message.get("error") {
            Some(error) => Err(error_message(error)),
            None => Ok(message
                .get_mut("result")
                .map(Value::take)
                .unwrap_or(Value::Null)),
        });
    }
    // The peer is gone: nothing will ever answer what is still in flight, and
    // taking the map is what makes a later request fail instead of wait.
    if let Some(in_flight) = pending.lock().await.take() {
        for (_, sender) in in_flight {
            let _ = sender.send(Err(CLOSED.to_string()));
        }
    }
}

/// Whether this is a header field name: non-empty, and a token throughout.
fn is_header_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b > b' ' && b < 0x7f && b != b'/')
}

/// The `message` a JSON-RPC error object carries.
fn error_message(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error")
        .to_string()
}

/// One `Content-Length`-framed JSON message, or `None` once the stream ends
/// or stops making sense (both mean the same to the caller: no more answers).
async fn read_message<R>(reader: &mut R) -> Option<Value>
where
    R: AsyncBufRead + AsyncRead + Unpin,
{
    let mut length = None;
    for _ in 0..MAX_HEADER_LINES {
        let mut line = String::new();
        if reader.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            let length = length?;
            if length > MAX_MESSAGE_BYTES {
                return None;
            }
            let mut payload = vec![0u8; length];
            reader.read_exact(&mut payload).await.ok()?;
            return serde_json::from_slice(&payload).ok();
        }
        // The plug-in's own reader matches the header name case-insensitively
        // on the part before the colon, and requires it to be a token; keep
        // the two agreeing on what a header line is, so neither can frame
        // something the other cannot read, and neither can be made to read a
        // request line as a header.
        let (name, value) = line.split_once(':')?;
        let name = name.trim();
        if !is_header_name(name) {
            return None;
        }
        if name.eq_ignore_ascii_case("Content-Length") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use tokio::net::TcpListener;

    /// Frame a payload the way the plug-in does.
    fn frame(payload: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{payload}", payload.len()).into_bytes()
    }

    fn write_descriptor(dir: &Path, body: &str) {
        let bridge_dir = dir.join(BRIDGE_DIR);
        std::fs::create_dir_all(&bridge_dir).unwrap();
        std::fs::write(bridge_dir.join(PORT_FILE), body).unwrap();
    }

    /// A workspace publishing a descriptor for `port`.
    fn workspace_for(prefix: &str, port: u16) -> TempDir {
        let dir = TempDir::new(prefix);
        write_descriptor(dir.path(), &format!(r#"{{"port":{port}}}"#));
        dir
    }

    /// A stand-in for the plug-in. `answer` decides the whole JSON-RPC message
    /// to send back for each request read; `None` closes the connection
    /// instead of answering. Returns the port and every request it read, so a
    /// test can see what the client actually sent.
    async fn fake_bridge<F>(answer: F) -> (u16, Arc<Mutex<Vec<Value>>>)
    where
        F: Fn(&Value) -> Option<Value> + Send + 'static,
    {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            while let Some(message) = read_message(&mut reader).await {
                recorded.lock().await.push(message.clone());
                let Some(reply) = answer(&message) else {
                    return;
                };
                write_half
                    .write_all(&frame(&reply.to_string()))
                    .await
                    .unwrap();
            }
        });
        (port, seen)
    }

    /// The ordinary plug-in: greets with `caps`, hands everything after that
    /// to `then`.
    fn greeting(
        caps: Value,
        then: impl Fn(&Value) -> Option<Value> + Send + 'static,
    ) -> impl Fn(&Value) -> Option<Value> + Send + 'static {
        move |message| {
            if message["method"] == "bridge/hello" {
                return Some(ok(
                    message,
                    json!({"protocol": 1, "rodin": "3.10.0", "caps": caps.clone()}),
                ));
            }
            then(message)
        }
    }

    fn ok(request: &Value, result: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": request["id"], "result": result})
    }

    fn failure(id: Value, code: i64, message: &str) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
    }

    /// The methods a recording saw, in order.
    async fn methods(seen: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
        seen.lock()
            .await
            .iter()
            .map(|message| message["method"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn reads_back_to_back_framed_messages() {
        let mut bytes = frame(r#"{"id":1,"result":{"a":1}}"#);
        bytes.extend(frame(r#"{"method":"bridge/shutdown"}"#));
        let mut reader = BufReader::new(&bytes[..]);

        assert_eq!(read_message(&mut reader).await.unwrap()["result"]["a"], 1);
        assert_eq!(
            read_message(&mut reader).await.unwrap()["method"],
            "bridge/shutdown"
        );
        assert!(read_message(&mut reader).await.is_none(), "then EOF");
    }

    #[tokio::test]
    async fn a_truncated_or_unparsable_message_ends_the_stream() {
        let full = frame(r#"{"id":1,"result":{}}"#);
        let mut truncated = BufReader::new(&full[..full.len() - 3]);
        assert!(read_message(&mut truncated).await.is_none());

        let garbage = frame("not json");
        assert!(
            read_message(&mut BufReader::new(&garbage[..]))
                .await
                .is_none()
        );

        let headerless = b"{}".to_vec();
        assert!(
            read_message(&mut BufReader::new(&headerless[..]))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_oversized_frame_is_refused_and_header_names_ignore_case() {
        let huge = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1).into_bytes();
        assert!(read_message(&mut BufReader::new(&huge[..])).await.is_none());

        // A header name matched the way the plug-in matches it.
        let mixed = b"content-length: 2\r\n\r\n{}".to_vec();
        assert_eq!(
            read_message(&mut BufReader::new(&mixed[..])).await.unwrap(),
            json!({})
        );
    }

    #[tokio::test]
    async fn a_request_line_is_not_a_header() {
        // "POST /a:b HTTP/1.1" splits at a colon, so without a rule about what
        // a header name may contain, an HTTP request a browser can be made to
        // send frames a message of the sender's choosing.
        let body = r#"{"id":1,"method":"project/register"}"#;
        let http = format!(
            "POST /a:b HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let bytes = http.into_bytes();
        assert!(
            read_message(&mut BufReader::new(&bytes[..]))
                .await
                .is_none()
        );
        assert!(is_header_name("Content-Length"));
        assert!(!is_header_name("POST /a") && !is_header_name(""));
    }

    #[tokio::test]
    async fn a_missing_or_unreadable_descriptor_is_an_error() {
        let dir = TempDir::new("rossi-bridge-descriptor");
        assert!(read_descriptor(dir.path()).is_err(), "nothing published");

        write_descriptor(dir.path(), "not json");
        assert!(read_descriptor(dir.path()).is_err());

        // Members this client does not read must not stop it reading the file.
        write_descriptor(
            dir.path(),
            r#"{"port":41235,"protocol":1,"pid":7,"extra":"ignored"}"#,
        );
        let descriptor = read_descriptor(dir.path()).unwrap();
        assert_eq!(descriptor.port, 41235);
        assert_eq!(descriptor.token, None);
    }

    #[tokio::test]
    async fn a_plugin_speaking_another_protocol_is_refused() {
        let (port, _) =
            fake_bridge(|message| Some(ok(message, json!({"protocol": 99, "caps": [REGISTER]}))))
                .await;
        let dir = workspace_for("rossi-bridge-protocol", port);

        let error = Bridge::connect(dir.path())
            .await
            .err()
            .expect("a protocol mismatch must not connect");
        assert!(error.contains("protocol 99"), "{error}");
    }

    #[tokio::test]
    async fn a_stale_descriptor_does_not_connect() {
        // A killed Rodin leaves its descriptor behind: the port is nobody's.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let dir = workspace_for("rossi-bridge-stale", port);
        assert!(Bridge::connect(dir.path()).await.is_err());
    }

    #[tokio::test]
    async fn connects_registers_and_reveals() {
        let (port, seen) = fake_bridge(greeting(json!([REGISTER, REVEAL]), |message| {
            Some(ok(message, json!({"project": "proj"})))
        }))
        .await;
        let dir = workspace_for("rossi-bridge-connect", port);

        let bridge = Bridge::connect(dir.path()).await.unwrap();
        assert_eq!(bridge.rodin_version(), "3.10.0");
        assert!(bridge.supports(REGISTER));

        let registered = bridge
            .register_project(Path::new("/ws/proj"))
            .await
            .unwrap();
        assert_eq!(registered.as_deref(), Some("proj"));
        bridge.reveal_project("proj").await.unwrap();

        assert_eq!(methods(&seen).await, ["bridge/hello", REGISTER, REVEAL]);
    }

    #[tokio::test]
    async fn a_plugin_without_the_method_reports_so() {
        // An older plug-in answers the handshake but offers less; the caller
        // must be able to see that before asking.
        let (port, _) = fake_bridge(greeting(json!([REVEAL]), |_| Some(json!({})))).await;
        let dir = workspace_for("rossi-bridge-caps", port);

        let bridge = Bridge::connect(dir.path()).await.unwrap();
        assert!(!bridge.supports(REGISTER));
        assert!(bridge.supports(REVEAL));
    }

    #[tokio::test]
    async fn presents_the_token_the_descriptor_publishes() {
        // The socket is loopback, which is not a boundary: another local user
        // can reach it, and a web page can be made to POST at it. Reading the
        // port file out of the user's own workspace is the proof the plug-in
        // asks for, so it has to actually leave here.
        let (port, seen) = fake_bridge(greeting(json!([REGISTER]), |_| Some(json!({})))).await;
        let dir = TempDir::new("rossi-bridge-token");
        write_descriptor(
            dir.path(),
            &format!(r#"{{"port":{port},"token":"c0ffee"}}"#),
        );

        Bridge::connect(dir.path()).await.unwrap();
        assert_eq!(seen.lock().await[0]["params"]["token"], "c0ffee");
    }

    #[tokio::test]
    async fn a_plugin_that_refuses_the_token_does_not_connect() {
        let (port, _) =
            fake_bridge(|message| Some(failure(message["id"].clone(), -32600, "bad token"))).await;
        let dir = workspace_for("rossi-bridge-bad-token", port);

        let error = Bridge::connect(dir.path())
            .await
            .err()
            .expect("a refused handshake must not connect");
        assert_eq!(error, "bad token");
    }

    #[tokio::test]
    async fn an_error_answer_becomes_the_reported_message() {
        let (port, _) = fake_bridge(greeting(json!([REGISTER]), |message| {
            Some(failure(
                message["id"].clone(),
                -32602,
                "no .project descriptor in /ws/proj",
            ))
        }))
        .await;
        let dir = workspace_for("rossi-bridge-error", port);

        let bridge = Bridge::connect(dir.path()).await.unwrap();
        let error = bridge
            .register_project(Path::new("/ws/proj"))
            .await
            .unwrap_err();
        assert_eq!(error, "no .project descriptor in /ws/proj");
    }

    #[tokio::test]
    async fn a_connection_that_drops_fails_the_request_in_flight() {
        // The peer reads the request and then goes, without answering.
        let (port, _) = fake_bridge(greeting(json!([REGISTER]), |_| None)).await;
        let dir = workspace_for("rossi-bridge-drop", port);

        let bridge = Bridge::connect(dir.path()).await.unwrap();
        let error = bridge
            .register_project(Path::new("/ws/proj"))
            .await
            .unwrap_err();
        assert_eq!(error, CLOSED);
    }

    #[tokio::test]
    async fn an_unattributable_error_fails_what_is_in_flight() {
        // A null id is how JSON-RPC reports a failure it cannot pin on a
        // request; the peer could not read what we sent, so waiting for an
        // answer to it is waiting for nothing.
        let (port, _) = fake_bridge(greeting(json!([REGISTER]), |_| {
            Some(failure(Value::Null, -32700, "could not parse that"))
        }))
        .await;
        let dir = workspace_for("rossi-bridge-null-id", port);

        let bridge = Bridge::connect(dir.path()).await.unwrap();
        let started = std::time::Instant::now();
        let error = bridge
            .register_project(Path::new("/ws/proj"))
            .await
            .unwrap_err();
        assert_eq!(error, "could not parse that");
        assert!(
            started.elapsed() < REQUEST_TIMEOUT,
            "must not wait out the request timeout"
        );
    }

    #[tokio::test]
    async fn a_request_made_after_the_peer_left_fails_at_once() {
        // Rodin quitting between the handshake and the ask is the ordinary
        // case: writing into a half-closed socket still succeeds, so without
        // the reader recording that it stopped this would wait out the whole
        // request timeout with the lens progress stuck on it. The peer has to
        // leave unprompted, so this one cannot go through `fake_bridge`.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            if let Some(message) = read_message(&mut reader).await {
                let answer = ok(&message, json!({"protocol": 1, "caps": [REGISTER]}));
                write_half
                    .write_all(&frame(&answer.to_string()))
                    .await
                    .unwrap();
            }
            // Go away for good, the way a closing Rodin does.
        });

        let dir = workspace_for("rossi-bridge-gone", port);
        let bridge = Bridge::connect(dir.path()).await.unwrap();
        // Let the reader observe the end of the stream before asking.
        while !bridge.transport.reader.is_finished() {
            tokio::task::yield_now().await;
        }

        let started = std::time::Instant::now();
        let error = bridge
            .register_project(Path::new("/ws/proj"))
            .await
            .unwrap_err();
        assert_eq!(error, CLOSED);
        assert!(
            started.elapsed() < REQUEST_TIMEOUT,
            "must not wait out the request timeout"
        );
    }
}
