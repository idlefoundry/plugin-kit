//! The bridge (the product's MCP binary): what an agent's MCP client starts. It answers MCP on its standard
//! input and output (`mcp.rs`), lists and chooses the instruments open on this computer that allow
//! agents (`discovery.rs`), and passes every other tool to the one chosen over its socket
//! (`server.rs`), connecting when first needed. Calls go to the only one open unless the agent
//! chooses; with several open and none chosen, a call is refused with their list. A instrument that
//! closes ends its connection: the next call says so, or (none having been chosen) connects to
//! the one then open, so that an agent's session outlives the application's restart.

// (Sockets are Unix's in this version: elsewhere the bridge and the server say so.)
#![cfg_attr(not(any(unix, windows)), allow(dead_code, unused_imports))]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

use super::discovery::{self, Record};
use super::mcp::Backend;
use super::server::TIMEOUT;
use super::tools::{Runs, Tool};
use super::{ApiError, PROTOCOL, Reply, codes, text_arg};

/// What an agent is told when no instrument listens.
pub const NONE_OPEN: &str = "No instrument allows agents on this computer. Open the instrument application and turn on SETTINGS > AGENTS: ALLOW (or start it with --agents), then try again.";

#[cfg(any(unix, windows))]
use super::Stream;

#[cfg(any(unix, windows))]
struct Connection {
    record: Record,
    out: Stream,
    reader: std::io::BufReader<Stream>,
}

/// The bridge's side of the conversation.
pub struct Bridge {
    dir: PathBuf,
    config: super::Config,
    /// The instance the agent chose (else the only one open).
    chosen: Option<String>,
    client: String,
    next: u64,
    #[cfg(any(unix, windows))]
    connection: Option<Connection>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("dir", &self.dir)
            .field("chosen", &self.chosen)
            .finish()
    }
}

impl Bridge {
    /// A bridge finding instruments in `dir`.
    pub fn at(dir: PathBuf, config: super::Config) -> Self {
        Bridge {
            dir,
            config,
            chosen: None,
            client: String::from("mcp-client"),
            next: 0,
            #[cfg(any(unix, windows))]
            connection: None,
        }
    }

    /// The instance chosen from the start (`--instance`).
    pub fn choose(&mut self, id: Option<String>) {
        self.chosen = id;
    }

    fn instances(&mut self) -> Reply {
        let live = discovery::live(&self.dir);
        let current = self.current_id();
        let list: Vec<Value> = live
            .iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "kind": r.kind,
                    "version": r.version,
                    "pid": r.pid,
                    "compatible": r.protocol == PROTOCOL,
                    "current": current.as_deref() == Some(r.id.as_str()),
                })
            })
            .collect();
        let mut v = json!({
            "instances": list,
            "chosen": self.chosen,
            "bridge_version": self.config.version,
        });
        if live.is_empty() {
            v["note"] = json!(NONE_OPEN);
        }
        Ok(v)
    }

    fn current_id(&self) -> Option<String> {
        #[cfg(any(unix, windows))]
        if let Some(c) = &self.connection {
            return Some(c.record.id.clone());
        }
        if self.chosen.is_some() {
            return self.chosen.clone();
        }
        let live = discovery::live(&self.dir);
        (live.len() == 1).then(|| live[0].id.clone())
    }

    fn select(&mut self, args: &Value) -> Reply {
        let id = text_arg(args, "instance")?;
        let live = discovery::live(&self.dir);
        let Some(r) = live.iter().find(|r| r.id == id || r.pid.to_string() == id) else {
            return Err(ApiError::new(
                codes::NO_INSTANCE,
                format!("no instrument `{id}` is open; the instances tool lists those that are"),
            ));
        };
        self.chosen = Some(r.id.clone());
        #[cfg(any(unix, windows))]
        if self
            .connection
            .as_ref()
            .is_some_and(|c| c.record.id != r.id)
        {
            self.connection = None;
        }
        Ok(json!({"chosen": r.id, "kind": r.kind, "version": r.version}))
    }

    /// The record of the instance to talk to.
    fn target(&self) -> Result<Record, ApiError> {
        let live = discovery::live(&self.dir);
        if let Some(id) = &self.chosen {
            return live.into_iter().find(|r| &r.id == id).ok_or_else(|| {
                ApiError::new(
                    codes::NO_INSTANCE,
                    format!(
                        "the instrument chosen ({id}) has closed; the instances tool lists those open"
                    ),
                )
            });
        }
        match live.len() {
            0 => Err(ApiError::new(codes::NO_INSTANCE, NONE_OPEN)),
            1 => Ok(live.into_iter().next().expect("one")),
            _ => Err(ApiError::new(
                codes::AMBIGUOUS,
                "several instruments are open: choose one with the select tool",
            )
            .with_data(json!({"instances": live.iter().map(|r| &r.id).collect::<Vec<_>>()}))),
        }
    }

    #[cfg(any(unix, windows))]
    fn connect(&mut self) -> Result<(), ApiError> {
        if self.connection.is_some() {
            return Ok(());
        }
        let record = self.target()?;
        if record.protocol != PROTOCOL {
            return Err(ApiError::refused(format!(
                "the instrument open is version {}, which this bridge ({}) cannot talk to: use the bridge that came with it",
                record.version, self.config.version
            )));
        }
        let gone = |e: std::io::Error| {
            ApiError::new(
                codes::NO_INSTANCE,
                format!("could not reach the instrument: {e}"),
            )
        };
        let out = Stream::connect(&record.endpoint).map_err(gone)?;
        out.set_read_timeout(Some(TIMEOUT + Duration::from_secs(5)))
            .map_err(gone)?;
        out.set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(gone)?;
        let reader = std::io::BufReader::new(out.try_clone().map_err(gone)?);
        self.connection = Some(Connection {
            record,
            out,
            reader,
        });
        let hello = self.exchange("hello", &json!({"name": self.client}));
        match hello {
            Ok(h) if h.get("protocol").and_then(Value::as_u64) == Some(PROTOCOL) => Ok(()),
            Ok(_) => {
                self.connection = None;
                Err(ApiError::refused("the instrument speaks another protocol"))
            }
            Err(e) => {
                self.connection = None;
                Err(e)
            }
        }
    }

    /// One request and its answer over the connection; the connection dropped if it fails.
    #[cfg(any(unix, windows))]
    fn exchange(&mut self, method: &str, params: &Value) -> Reply {
        use std::io::{BufRead, Write};
        self.next += 1;
        let id = self.next;
        let Some(c) = self.connection.as_mut() else {
            return Err(ApiError::new(codes::NO_INSTANCE, NONE_OPEN));
        };
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let lost = |e: String| {
            ApiError::new(
                codes::NO_INSTANCE,
                format!("the instrument has closed ({e})"),
            )
        };
        let result = (|| {
            writeln!(c.out, "{msg}").map_err(|e| lost(e.to_string()))?;
            c.out.flush().map_err(|e| lost(e.to_string()))?;
            loop {
                let mut line = String::new();
                let n = std::io::Read::take(&mut c.reader, super::mcp::MAX_LINE as u64 + 1)
                    .read_line(&mut line)
                    .map_err(|e| {
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) {
                            ApiError::new(codes::TIMEOUT, "the instrument did not answer in time")
                        } else {
                            lost(e.to_string())
                        }
                    })?;
                if n > super::mcp::MAX_LINE {
                    return Err(lost("answer too large".into()));
                }
                if n == 0 {
                    return Err(lost("its connection ended".into()));
                }
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if v.get("id") != Some(&json!(id)) {
                    // (Refused at connecting, with no id: too many agents.)
                    if v.get("id") == Some(&Value::Null)
                        && let Some(e) = v.get("error")
                    {
                        return Err(ApiError::from_json(e));
                    }
                    continue;
                }
                return match (v.get("result"), v.get("error")) {
                    (_, Some(e)) => Err(ApiError::from_json(e)),
                    (Some(r), None) => Ok(r.clone()),
                    (None, None) => Err(ApiError::new(codes::INTERNAL, "an empty answer")),
                };
            }
        })();
        if let Err(e) = &result
            && matches!(e.code, codes::NO_INSTANCE | codes::TIMEOUT | codes::BUSY)
        {
            self.connection = None;
        }
        result
    }

    #[cfg(any(unix, windows))]
    fn forward(&mut self, method: &str, args: &Value) -> Reply {
        let had = self.connection.is_some();
        self.connect()?;
        match self.exchange(method, args) {
            // A connection that was there before this call and is gone now (the application
            // restarted): once more, to the one now open.
            Err(e) if had && e.code == codes::NO_INSTANCE && self.connection.is_none() => {
                self.connect()?;
                self.exchange(method, args)
            }
            r => r,
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn forward(&mut self, _method: &str, _args: &Value) -> Reply {
        Err(ApiError::new(
            codes::NO_INSTANCE,
            "agents are not yet supported on this system",
        ))
    }
}

impl Backend for Bridge {
    fn call(&mut self, tool: &Tool, args: &Value) -> Reply {
        match (tool.runs, tool.method) {
            (Runs::Bridge, "instances") => self.instances(),
            (Runs::Bridge, "select") => self.select(args),
            (_, method) => self.forward(method, args),
        }
    }

    fn client(&mut self, name: &str) {
        self.client = name.chars().take(64).collect();
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;
    use crate::server::Server;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn answering(s: &Server) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let inbox = Arc::clone(s.inbox());
        let stop = Arc::new(AtomicBool::new(false));
        let st = Arc::clone(&stop);
        let h = std::thread::spawn(move || {
            while !st.load(Ordering::Acquire) {
                while let Some(job) = inbox.take() {
                    let r = if job.method == "parameters.get" {
                        Err(ApiError::params("bad").with_data(json!({"min": 0})))
                    } else {
                        Ok(json!({"did": job.method, "client": job.client, "args": job.params}))
                    };
                    job.answer(r);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        (stop, h)
    }

    fn tool(name: &str) -> Tool {
        crate::test_config()
            .tools
            .into_iter()
            .find(|t| t.name == name)
            .expect(name)
    }

    /// None open: told how to allow agents. One: used. Two: refused until one is chosen.
    /// An instance's errors come back as they were; one closed is said to be.
    #[test]
    fn instances_are_found_chosen_and_lost() {
        let t = tempfile::tempdir().expect("tempdir");
        let dir = t.path().join("agents");
        let mut b = Bridge::at(dir.clone(), crate::test_config());
        b.client("tester");
        let err = b
            .call(&tool("test_describe"), &json!({}))
            .expect_err("none");
        assert_eq!(err.code, codes::NO_INSTANCE);
        assert!(err.message.contains("AGENTS"));
        let one = Server::start_in(&dir, "standalone", "standalone-1", crate::test_config())
            .expect("one");
        let (stop1, h1) = answering(&one);
        let r = b
            .call(&tool("test_describe"), &json!({}))
            .expect("describe");
        assert_eq!(r["did"], json!("describe"));
        assert_eq!(r["client"], json!("tester"));
        let e = b
            .call(&tool("test_get"), &json!({}))
            .expect_err("instance's error");
        assert_eq!(
            (e.code, e.data),
            (codes::INVALID_PARAMS, Some(json!({"min": 0})))
        );
        let list = b.call(&tool("test_instances"), &json!({})).expect("list");
        assert_eq!(list["instances"][0]["current"], json!(true));
        // A second: the first stays in use while connected; a new bridge must choose.
        let two = Server::start_in(&dir, "standalone", "standalone-2", crate::test_config())
            .expect("two");
        let (stop2, h2) = answering(&two);
        assert!(b.call(&tool("test_describe"), &json!({})).is_ok());
        let mut fresh = Bridge::at(dir.clone(), crate::test_config());
        let e = fresh
            .call(&tool("test_describe"), &json!({}))
            .expect_err("ambiguous");
        assert_eq!(e.code, codes::AMBIGUOUS);
        assert!(
            fresh
                .call(&tool("test_select"), &json!({"instance": "nope"}))
                .is_err()
        );
        fresh
            .call(&tool("test_select"), &json!({"instance": "standalone-2"}))
            .expect("select");
        assert!(fresh.call(&tool("test_describe"), &json!({})).is_ok());
        // The chosen one closes: said, not passed to another.
        stop2.store(true, Ordering::Release);
        h2.join().expect("two's editor");
        drop(two);
        let e = fresh
            .call(&tool("test_describe"), &json!({}))
            .expect_err("closed");
        assert_eq!(e.code, codes::NO_INSTANCE);
        // The first bridge, none chosen, closes and reopens: it reconnects to the one open.
        stop1.store(true, Ordering::Release);
        h1.join().expect("one's editor");
        drop(one);
        let again = Server::start_in(&dir, "standalone", "standalone-3", crate::test_config())
            .expect("three");
        let (stop3, h3) = answering(&again);
        let r = b
            .call(&tool("test_describe"), &json!({}))
            .expect("reconnected");
        assert_eq!(r["did"], json!("describe"));
        stop3.store(true, Ordering::Release);
        h3.join().expect("three's editor");
    }
}
