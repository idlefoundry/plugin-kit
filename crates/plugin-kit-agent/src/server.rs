//! A instrument's socket for agents: JSON-RPC 2.0, one message a line, from the bridges of the
//! user's agents (at most [`MAX_CLIENTS`] at once). A connection's requests are answered in
//! order, one at a time. `hello` and `ping` are answered here; every other method is handed to
//! the editor through the [`Inbox`], which its window takes from at each frame, on its own
//! thread, and answered when it has been carried out there.
//!
//! Nothing here waits on the audio thread or the editor's locks: the window takes a request
//! when it next draws (every 15 ms or so, every 250 ms while unseen), and a request it has not
//! taken within [`TIMEOUT`] is answered as timed out and never carried out. A request whose
//! client has gone before the window takes it is dropped; one it has taken is carried out, its
//! answer let go. The socket and its record are removed as the server is dropped (the editor
//! closing, or agents turned off).

// (Sockets are Unix's in this version: elsewhere the bridge and the server say so.)
#![cfg_attr(not(any(unix, windows)), allow(dead_code, unused_imports))]

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::discovery::{self, Record};
use super::mcp::MAX_LINE;
use super::{ApiError, Config, PROTOCOL, Reply, codes};

/// How long the editor has to take a request.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// How many bridges may be connected at once.
pub const MAX_CLIENTS: usize = 8;
/// How often a waiting thread looks up (the server stopping, its client gone).
const SLICE: Duration = Duration::from_millis(100);

const WAITING: u8 = 0;
const TAKEN: u8 = 1;
const DROPPED: u8 = 2;

/// A request for the editor to carry out.
#[derive(Debug)]
pub struct Job {
    pub method: String,
    pub params: Value,
    /// The client's name, as its `hello` gave it.
    pub client: String,
    state: Arc<AtomicU8>,
    reply: SyncSender<Reply>,
}

impl Job {
    pub fn answer(self, r: Reply) {
        let _ = self.reply.try_send(r);
    }
}

/// The requests waiting for the editor.
#[derive(Debug, Default)]
pub struct Inbox {
    jobs: Mutex<VecDeque<Job>>,
    closed: AtomicBool,
}

impl Inbox {
    /// The next request to carry out, if any; one its client has given up on is skipped.
    pub fn take(&self) -> Option<Job> {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while let Some(job) = jobs.pop_front() {
            if job
                .state
                .compare_exchange(WAITING, TAKEN, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(job);
            }
        }
        None
    }

    /// Whether `job` is waiting now (not once the inbox has closed).
    fn push(&self, job: Job) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        jobs.retain(|j| j.state.load(Ordering::Acquire) != DROPPED);
        if jobs.len() >= 32 {
            return false;
        }
        jobs.push_back(job);
        true
    }

    /// No more requests: those waiting are let go (their clients are told).
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// A request, as a client's connection hands it over: its answer, or why there is none.
    /// `gone` says whether the client has gone meanwhile.
    pub fn ask(
        &self,
        method: &str,
        params: Value,
        client: &str,
        stop: &AtomicBool,
        mut gone: impl FnMut() -> bool,
    ) -> Reply {
        let (tx, rx) = sync_channel(1);
        let state = Arc::new(AtomicU8::new(WAITING));
        let job = Job {
            method: method.to_owned(),
            params,
            client: client.to_owned(),
            state: Arc::clone(&state),
            reply: tx,
        };
        if !self.push(job) {
            return Err(ApiError::new(
                codes::NO_INSTANCE,
                "the instrument is no longer listening",
            ));
        }
        wait(&rx, &state, stop, &mut gone)
    }
}

fn wait(
    rx: &Receiver<Reply>,
    state: &AtomicU8,
    stop: &AtomicBool,
    gone: &mut dyn FnMut() -> bool,
) -> Reply {
    let began = Instant::now();
    loop {
        match rx.recv_timeout(SLICE) {
            Ok(r) => return r,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(ApiError::new(
                    codes::NO_INSTANCE,
                    "the instrument stopped listening before it answered",
                ));
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        // Given up on, unless the editor has taken it (then its answer comes this frame).
        let give_up = |state: &AtomicU8| {
            state
                .compare_exchange(WAITING, DROPPED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        };
        if stop.load(Ordering::Acquire) {
            let _ = give_up(state);
            return Err(ApiError::new(
                codes::NO_INSTANCE,
                "the instrument stopped listening",
            ));
        }
        if gone() && give_up(state) {
            return Err(ApiError::new(codes::NO_INSTANCE, "the client has gone"));
        }
        if began.elapsed() >= TIMEOUT && give_up(state) {
            return Err(ApiError::new(
                codes::TIMEOUT,
                format!(
                    "the instrument's window did not take the request within {} s; nothing was done",
                    TIMEOUT.as_secs()
                ),
            ));
        }
    }
}

/// A instrument listening for agents.
#[derive(Debug)]
pub struct Server {
    inbox: Arc<Inbox>,
    stop: Arc<AtomicBool>,
    clients: Arc<AtomicUsize>,
    listener: Mutex<Option<std::thread::JoinHandle<()>>>,
    record: Record,
    dir: PathBuf,
    _awake: super::awake::Awake,
}

impl Server {
    /// Listening in `dir`, as `id`.
    #[cfg(any(unix, windows))]
    pub fn start_in(dir: &Path, kind: &str, id: &str, config: Config) -> Result<Server, String> {
        use super::Listener;
        if !discovery::valid_id(id) {
            return Err(
                "instance id must use 1..64 ASCII letters, digits, hyphens or underscores".into(),
            );
        }
        discovery::ensure(dir)?;
        let path = discovery::socket_path(dir, id);
        // (One left by a instrument of this process's number that did not stop cleanly.)
        let _ = std::fs::remove_file(&path);
        let listener = Listener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        // (On Windows the socket takes its folder's access, the user's alone.)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let record = Record {
            id: id.to_owned(),
            pid: std::process::id(),
            kind: kind.to_owned(),
            version: config.version.to_owned(),
            protocol: PROTOCOL,
            endpoint: path.to_string_lossy().into_owned(),
            started: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        };
        let inbox = Arc::new(Inbox::default());
        let stop = Arc::new(AtomicBool::new(false));
        let clients = Arc::new(AtomicUsize::new(0));
        let handle = {
            let (inbox, stop, clients, record) = (
                Arc::clone(&inbox),
                Arc::clone(&stop),
                Arc::clone(&clients),
                record.clone(),
            );
            std::thread::Builder::new()
                .name("instrument agents".into())
                .spawn(move || listen(listener, &inbox, &stop, &clients, &record, &config))
                .map_err(|e| e.to_string())?
        };
        let server = Server {
            inbox,
            stop,
            clients,
            listener: Mutex::new(Some(handle)),
            record,
            dir: dir.to_owned(),
            _awake: super::awake::Awake::begin(),
        };
        discovery::publish(dir, &server.record)?;
        Ok(server)
    }

    #[cfg(not(any(unix, windows)))]
    pub fn start_in(_dir: &Path, _kind: &str, _id: &str, config: Config) -> Result<Server, String> {
        Err("agents are not yet supported on this system".into())
    }

    pub fn inbox(&self) -> &Arc<Inbox> {
        &self.inbox
    }

    pub fn record(&self) -> &Record {
        &self.record
    }

    /// How many bridges are connected.
    pub fn clients(&self) -> usize {
        self.clients.load(Ordering::Acquire)
    }
}

/// This process's id as a instrument of this `kind` listening for agents.
pub fn own_id(kind: &str) -> String {
    format!("{kind}-{}", std::process::id())
}

impl Server {
    /// End all transport work even if an editor handle is still retained by its host.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        self.inbox.close();
        if let Some(h) = self
            .listener
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(discovery::record_path(&self.dir, &self.record.id));
        let _ = std::fs::remove_file(&self.record.endpoint);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(any(unix, windows))]
fn listen(
    listener: super::Listener,
    inbox: &Arc<Inbox>,
    stop: &Arc<AtomicBool>,
    clients: &Arc<AtomicUsize>,
    record: &Record,
    config: &Config,
) {
    use std::io::Write;
    let mut threads: Vec<std::thread::JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                // Bound BUSY on the listener too, before any thread can write.
                if stream
                    .set_write_timeout(Some(Duration::from_millis(200)))
                    .is_err()
                {
                    continue;
                }
                let mut i = 0;
                while i < threads.len() {
                    if threads[i].is_finished() {
                        let _ = threads.swap_remove(i).join();
                    } else {
                        i += 1;
                    }
                }
                if clients.load(Ordering::Acquire) >= MAX_CLIENTS {
                    let e = ApiError::new(
                        codes::BUSY,
                        format!("the instrument already has {MAX_CLIENTS} agents connected"),
                    );
                    let _ = writeln!(stream, "{}", response(Value::Null, Err(e)));
                    continue;
                }
                clients.fetch_add(1, Ordering::AcqRel);
                let failed_clients = Arc::clone(clients);
                let (inbox, stop, clients, record) = (
                    Arc::clone(inbox),
                    Arc::clone(stop),
                    Arc::clone(clients),
                    record.clone(),
                );
                let config = config.clone();
                let spawned = std::thread::Builder::new()
                    .name("instrument agent".into())
                    .spawn(move || {
                        connection(stream, &inbox, &stop, &record, &config);
                        clients.fetch_sub(1, Ordering::AcqRel);
                    });
                match spawned {
                    Ok(t) => threads.push(t),
                    Err(_) => {
                        failed_clients.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    for t in threads {
        let _ = t.join();
    }
}

fn response(id: Value, r: Reply) -> Value {
    match r {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e.to_json()}),
    }
}

/// One client's connection, until it closes or the server stops.
#[cfg(any(unix, windows))]
fn connection(
    stream: super::Stream,
    inbox: &Inbox,
    stop: &AtomicBool,
    record: &Record,
    config: &Config,
) {
    use std::io::{Read, Write};
    if stream.set_nonblocking(false).is_err()
        || stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .is_err()
    {
        return;
    }
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let _ = out.set_write_timeout(Some(Duration::from_millis(200)));
    let mut stream = stream;
    let mut buf: Vec<u8> = Vec::new();
    let mut client = String::from("agent");
    let mut chunk = [0u8; 8192];
    while !stop.load(Ordering::Acquire) {
        let Some(end) = buf.iter().position(|b| *b == b'\n') else {
            if buf.len() > MAX_LINE {
                let e = ApiError::new(codes::INVALID_REQUEST, "the message is too long");
                let _ = writeln!(out, "{}", response(Value::Null, Err(e)));
                return;
            }
            match stream.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return,
            }
            continue;
        };
        if end >= MAX_LINE {
            let _ = writeln!(
                out,
                "{}",
                response(
                    Value::Null,
                    Err(ApiError::new(codes::INVALID_REQUEST, "message too long"))
                )
            );
            return;
        }
        let line: Vec<u8> = buf.drain(..=end).collect();
        let text = String::from_utf8_lossy(&line).into_owned();
        if text.trim().is_empty() {
            continue;
        }
        // While the editor carries it out: the client's next bytes kept, its close noticed.
        let mut pending = Vec::new();
        let mut closed = false;
        let gone = || {
            if !closed {
                closed = client_gone(&stream, &mut pending);
            }
            closed
        };
        let answer = request(&text, inbox, stop, record, config, &mut client, gone);
        buf.extend_from_slice(&pending);
        if let Some(answer) = answer
            && writeln!(out, "{answer}")
                .and_then(|()| out.flush())
                .is_err()
        {
            return;
        }
        if closed {
            return;
        }
    }
}

/// Whether the client has closed its end, read without waiting; what it sent meanwhile (its
/// next request) kept in `pending`.
#[cfg(any(unix, windows))]
fn client_gone(s: &super::Stream, pending: &mut Vec<u8>) -> bool {
    use std::io::Read;
    if s.set_nonblocking(true).is_err() {
        return false;
    }
    let mut chunk = [0u8; 4096];
    let mut r = s;
    let closed = loop {
        match r.read(&mut chunk) {
            Ok(0) => break true,
            Ok(n) => {
                pending.extend_from_slice(&chunk[..n]);
                if pending.len() > MAX_LINE {
                    break true;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => break e.kind() != std::io::ErrorKind::WouldBlock,
        }
    };
    let _ = s.set_nonblocking(false);
    closed
}

/// One line's request answered (none for a notification).
fn request(
    text: &str,
    inbox: &Inbox,
    stop: &AtomicBool,
    record: &Record,
    config: &Config,
    client: &mut String,
    gone: impl FnMut() -> bool,
) -> Option<Value> {
    let msg: Value = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            return Some(response(
                Value::Null,
                Err(ApiError::new(codes::PARSE_ERROR, e.to_string())),
            ));
        }
    };
    let id = msg.get("id").cloned();
    let invalid = |m: &str| {
        Some(response(
            id.clone().unwrap_or(Value::Null),
            Err(ApiError::new(codes::INVALID_REQUEST, m)),
        ))
    };
    if !msg.is_object() || msg.get("jsonrpc") != Some(&json!("2.0")) {
        return invalid("expected a JSON-RPC 2.0 object");
    }
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return invalid("missing method");
    };
    let id = id?;
    if !id.is_null() && !id.is_string() && !id.is_number() {
        return Some(response(
            Value::Null,
            Err(ApiError::new(codes::INVALID_REQUEST, "invalid request id")),
        ));
    }
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let r = match method {
        "hello" => {
            if let Some(n) = params.get("name").and_then(Value::as_str) {
                *client = n.chars().take(64).collect();
            }
            Ok(json!({
                "name": config.name,
                "id": record.id,
                "kind": record.kind,
                "version": record.version,
                "protocol": PROTOCOL,
                "pid": record.pid,
            }))
        }
        "ping" => Ok(json!({})),
        m if config
            .tools
            .iter()
            .any(|t| t.runs == super::tools::Runs::Instance && t.method == m) =>
        {
            if !params.is_object() {
                Err(ApiError::params("`params` must be an object"))
            } else {
                inbox.ask(m, params, client, stop, gone)
            }
        }
        other => Err(ApiError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method `{other}`"),
        )),
    };
    Some(response(id, r))
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;
    use crate::Stream;
    use std::io::Read;
    use std::io::{BufRead, BufReader, Write};

    fn server() -> (tempfile::TempDir, Server) {
        let t = tempfile::tempdir().expect("tempdir");
        let s = Server::start_in(
            &t.path().join("a"),
            "standalone",
            "standalone-test",
            crate::test_config(),
        )
        .expect("start");
        (t, s)
    }

    fn connect(s: &Server) -> (Stream, BufReader<Stream>) {
        let c = Stream::connect(&s.record().endpoint).expect("connect");
        let r = BufReader::new(c.try_clone().expect("clone"));
        (c, r)
    }

    fn line(r: &mut BufReader<Stream>) -> Value {
        let mut l = String::new();
        r.read_line(&mut l).expect("read");
        serde_json::from_str(&l).expect("json")
    }

    /// The editor's side, for the tests: takes requests and answers each with its method.
    fn editor(inbox: Arc<Inbox>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<usize> {
        std::thread::spawn(move || {
            let mut n = 0;
            while !stop.load(Ordering::Acquire) {
                while let Some(job) = inbox.take() {
                    n += 1;
                    let m = job.method.clone();
                    job.answer(Ok(json!({"did": m})));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            n
        })
    }

    /// Hello and ping answered by the server, a method by the editor, unknown ones refused;
    /// the socket and record are the user's alone and go with the server.
    #[test]
    fn requests_reach_the_editor_and_are_answered_in_order() {
        let (_t, s) = server();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&s.record().endpoint)
                .expect("socket")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
        let stop = Arc::new(AtomicBool::new(false));
        let ed = editor(Arc::clone(s.inbox()), Arc::clone(&stop));
        let (mut c, mut r) = connect(&s);
        for m in [
            r#"{"jsonrpc":"2.0","id":1,"method":"hello","params":{"name":"test"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"describe"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"rm -rf"}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"describe","params":[1]}"#,
            r#"not json"#,
            r#"{"id":5,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"ping"}"#,
        ] {
            writeln!(c, "{m}").expect("write");
        }
        let hello = line(&mut r);
        assert_eq!(hello["result"]["protocol"], json!(PROTOCOL));
        assert_eq!(hello["result"]["id"], json!("standalone-test"));
        assert_eq!(line(&mut r)["result"]["did"], json!("describe"));
        assert_eq!(
            line(&mut r)["error"]["code"],
            json!(codes::METHOD_NOT_FOUND)
        );
        assert_eq!(line(&mut r)["error"]["code"], json!(codes::INVALID_PARAMS));
        assert_eq!(line(&mut r)["error"]["code"], json!(codes::PARSE_ERROR));
        assert_eq!(line(&mut r)["error"]["code"], json!(codes::INVALID_REQUEST));
        assert_eq!(line(&mut r)["id"], json!(6));
        stop.store(true, Ordering::Release);
        assert_eq!(ed.join().expect("editor"), 1);
        let (sock, rec) = (
            PathBuf::from(&s.record().endpoint),
            discovery::record_path(&s.dir, &s.record().id),
        );
        assert!(sock.exists() && rec.exists());
        drop(s);
        assert!(!sock.exists() && !rec.exists());
    }

    /// A request the editor does not take in time is answered as timed out, and is never
    /// carried out afterwards.
    #[test]
    fn a_request_not_taken_is_dropped() {
        let inbox = Inbox::default();
        let stop = AtomicBool::new(false);
        let mut asked = 0;
        // (Its client goes at the third look: as a timeout, without waiting ten seconds.)
        let r = inbox.ask("describe", json!({}), "t", &stop, || {
            asked += 1;
            asked >= 3
        });
        assert_eq!(r.expect_err("gone").code, codes::NO_INSTANCE);
        assert!(
            inbox.take().is_none(),
            "a dropped request is not carried out"
        );
    }

    /// A client gone mid-request: the editor's answer is let go, and the server goes on
    /// serving others.
    #[test]
    fn a_client_gone_mid_request_leaves_the_server_serving() {
        let (_t, s) = server();
        {
            let (mut c, _r) = connect(&s);
            writeln!(c, r#"{{"jsonrpc":"2.0","id":1,"method":"describe"}}"#).expect("write");
            // Gone before the editor takes it.
        }
        std::thread::sleep(Duration::from_millis(400));
        assert!(s.inbox().take().is_none(), "dropped: its client has gone");
        // One taken, then its client gone: carried out, its answer let go.
        {
            let (mut c, _r) = connect(&s);
            writeln!(c, r#"{{"jsonrpc":"2.0","id":2,"method":"describe"}}"#).expect("write");
            let began = Instant::now();
            let job = loop {
                if let Some(j) = s.inbox().take() {
                    break j;
                }
                assert!(began.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(5));
            };
            drop(c);
            job.answer(Ok(json!({})));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let ed = editor(Arc::clone(s.inbox()), Arc::clone(&stop));
        let (mut c, mut r) = connect(&s);
        writeln!(c, r#"{{"jsonrpc":"2.0","id":3,"method":"describe"}}"#).expect("write");
        assert_eq!(line(&mut r)["id"], json!(3));
        stop.store(true, Ordering::Release);
        ed.join().expect("editor");
    }

    /// No more than MAX_CLIENTS at once: the next is told so.
    #[test]
    fn too_many_clients_are_refused() {
        let (_t, s) = server();
        let mut held = Vec::new();
        for i in 0..MAX_CLIENTS {
            let (mut c, mut r) = connect(&s);
            writeln!(c, r#"{{"jsonrpc":"2.0","id":{i},"method":"ping"}}"#).expect("write");
            assert_eq!(line(&mut r)["id"], json!(i));
            held.push((c, r));
        }
        let (_c, mut r) = connect(&s);
        assert_eq!(line(&mut r)["error"]["code"], json!(codes::BUSY));
        assert_eq!(s.clients(), MAX_CLIENTS);
    }

    /// The server stopping ends a request waiting for the editor at once.
    #[test]
    fn stopping_answers_what_waits() {
        let (_t, s) = server();
        let (mut c, mut r) = connect(&s);
        writeln!(c, r#"{{"jsonrpc":"2.0","id":1,"method":"describe"}}"#).expect("write");
        std::thread::sleep(Duration::from_millis(100));
        let began = Instant::now();
        drop(s);
        assert!(began.elapsed() < Duration::from_secs(2));
        let mut l = String::new();
        let _ = r.read_line(&mut l);
        if !l.is_empty() {
            let v: Value = serde_json::from_str(&l).expect("json");
            assert_eq!(v["error"]["code"], json!(codes::NO_INSTANCE));
        }
    }
    #[test]
    fn shutdown_joins_workers_even_with_a_taken_deferred_job() {
        let (_t, s) = server();
        let (mut c, mut r) = connect(&s);
        writeln!(c, r#"{{"jsonrpc":"2.0","id":1,"method":"describe"}}"#).expect("write");
        let began = Instant::now();
        let job = loop {
            if let Some(job) = s.inbox().take() {
                break job;
            }
            assert!(began.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(5));
        };
        s.shutdown();
        assert!(began.elapsed() < Duration::from_secs(3));
        assert_eq!(line(&mut r)["error"]["code"], json!(codes::NO_INSTANCE));
        assert!(!std::path::Path::new(&s.record().endpoint).exists());
        drop(job);
        s.shutdown();
    }
    #[test]
    fn unsafe_instance_ids_are_refused() {
        let t = tempfile::tempdir().expect("tempdir");
        assert!(
            Server::start_in(t.path(), "standalone", "../outside", crate::test_config()).is_err()
        );
    }
    /// A bridge that stops reading a large answer must not hold the editor open on quit.
    #[test]
    fn stopping_does_not_wait_for_a_client_to_read_its_answer() {
        let (_t, s) = server();
        let (mut c, _r) = connect(&s);
        c.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        c.set_write_timeout(Some(Duration::from_secs(5)))
            .expect("write timeout");
        let id = "x".repeat(700 * 1024);
        let ping = json!({"jsonrpc": "2.0", "id": id, "method": "ping"});
        writeln!(c, "{ping}").expect("ping");
        // The answer has started. Leave the remaining bytes unread and the peer open.
        c.read_exact(&mut [0u8; 1]).expect("answer starts");
        let (done, result) = sync_channel(1);
        let shutdown = std::thread::spawn(move || {
            drop(s);
            let _ = done.send(());
        });
        let stopped = result.recv_timeout(Duration::from_secs(2)).is_ok();
        // Also release a broken implementation before asserting, so the test cannot hang.
        let _ = c.shutdown(std::net::Shutdown::Both);
        shutdown.join().expect("shutdown thread");
        assert!(stopped, "shutdown waited for a client that was not reading");
    }

    #[test]
    fn a_client_reading_a_large_answer_receives_it_whole() {
        let (_t, s) = server();
        let (mut c, mut r) = connect(&s);
        c.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        c.set_write_timeout(Some(Duration::from_secs(5)))
            .expect("write timeout");
        let id = "x".repeat(700 * 1024);
        let ping = json!({"jsonrpc": "2.0", "id": id, "method": "ping"});
        writeln!(c, "{ping}").expect("ping");
        let answer = line(&mut r);
        assert_eq!(answer["id"], ping["id"]);
        assert_eq!(answer["result"], json!({}));
    }
}
