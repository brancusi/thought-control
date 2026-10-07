//! `caretline send`: a small client for the state protocol.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use caretline_next::trace::parse_msgs;
use clap::Parser;
use serde_json::{json, Value};

use crate::hub::discovery_dir;

#[derive(Parser, Debug)]
#[command(
    name = "caretline send",
    about = "Send state-protocol requests to a running caretline and print the responses",
    after_help = "REQUEST is a JSON request, or one of:\n  hello | state.get | unsubscribe | trace.checkpoint\n  trace.get [all | since REV]\n  render WxH [text|ansi|cells]\n  keys SCRIPT\n  msgs FILE|-|JSON\n  set-state FILE|-\n  status TEXT              (a message in the editor's status bar)\n  subscribe [WxH]          (streams events until interrupted)\nWithout REQUEST, reads JSON requests from stdin, one per line.\n\nExamples:\n  caretline send --latest state.get\n  caretline send --latest render 80x24 --raw\n  caretline send --latest keys '<down>hello'\n  caretline send --pid 4242 set-state s.json\n  caretline send --socket /tmp/cl.sock '{\"id\":1,\"op\":\"hello\"}'"
)]
struct SendArgs {
    /// The server's socket.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["pid", "latest"])]
    socket: Option<PathBuf>,
    /// The editor with this process id (from its discovery file).
    #[arg(long, value_name = "N", conflicts_with = "latest")]
    pid: Option<u32>,
    /// The most recently started editor that answers (the default).
    #[arg(long)]
    latest: bool,
    /// Print the payload instead of the JSON response: the frame for render, the state for
    /// state.get, the trace (JSON Lines) for trace.get.
    #[arg(long)]
    raw: bool,
    /// Perform the effects of pushed messages in the editor (saves, the clipboard).
    #[arg(long)]
    apply_effects: bool,
    /// The request and its arguments.
    request: Vec<String>,
}

/// Finds the socket a discovery file names.
fn from_discovery(path: &std::path::Path) -> Option<PathBuf> {
    let info: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    info["socket"].as_str().map(PathBuf::from)
}

fn resolve(args: &SendArgs) -> Result<PathBuf, String> {
    if let Some(s) = &args.socket {
        return Ok(s.clone());
    }
    let dir = discovery_dir();
    if let Some(pid) = args.pid {
        let file = dir.join(format!("{pid}.json"));
        return from_discovery(&file).ok_or_else(|| format!("no caretline with pid {pid} ({} not found)", file.display()));
    }
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .map_err(|_| format!("no running caretline found (nothing in {})", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, file) in found {
        let Some(socket) = from_discovery(&file) else { continue };
        if UnixStream::connect(&socket).is_ok() {
            return Ok(socket);
        }
        // Its editor is gone: tidy up after it.
        let _ = std::fs::remove_file(&file);
    }
    Err("no running caretline found (start one with: caretline FILE --listen)".into())
}

fn read_arg(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s).map_err(|e| format!("stdin: {e}"))?;
        Ok(s)
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
    }
}

fn size(s: &str) -> Result<(u16, u16), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or_else(|| format!("bad size {s:?}: expected WxH"))?;
    Ok((
        w.parse().map_err(|_| format!("bad width in {s:?}"))?,
        h.parse().map_err(|_| format!("bad height in {s:?}"))?,
    ))
}

/// Turns the request words into a JSON request.
fn build(words: &[String], apply_effects: bool) -> Result<Value, String> {
    let op = words[0].as_str();
    let arg = |i: usize, what: &str| words.get(i).cloned().ok_or_else(|| format!("{op} needs {what}"));
    let mut req = match op {
        s if s.trim_start().starts_with('{') => {
            serde_json::from_str(s).map_err(|e| format!("request: {e}"))?
        }
        "hello" | "state.get" | "unsubscribe" | "trace.checkpoint" => json!({ "op": op }),
        "trace.get" => match words.get(1).map(String::as_str) {
            None => json!({ "op": op }),
            Some("all") => json!({ "op": op, "all": true }),
            Some("since") => {
                let rev: u64 = arg(2, "a rev")?.parse().map_err(|_| "since needs a rev number".to_string())?;
                json!({ "op": op, "since_rev": rev })
            }
            Some(other) => return Err(format!("trace.get takes all or since REV, not {other:?}")),
        },
        "render" => {
            let mut r = json!({ "op": "render" });
            if let Some(s) = words.get(1) {
                let (w, h) = size(s)?;
                r["w"] = w.into();
                r["h"] = h.into();
            }
            if let Some(f) = words.get(2) {
                r["format"] = f.as_str().into();
            }
            r
        }
        "keys" => json!({ "op": "keys", "keys": arg(1, "a key script")? }),
        "msgs" => {
            let src = arg(1, "a file, - or JSON")?;
            let text = if src.trim_start().starts_with(['{', '[']) { src } else { read_arg(&src)? };
            json!({ "op": "msgs", "msgs": parse_msgs(&text)? })
        }
        "set-state" | "state.set" => {
            let text = read_arg(&arg(1, "a state file or -")?)?;
            let state: Value = serde_json::from_str(&text).map_err(|e| format!("state: {e}"))?;
            json!({ "op": "state.set", "state": state })
        }
        "status" => json!({ "op": "msgs", "msgs": [{ "msg": "show_status", "text": words[1..].join(" ") }] }),
        "subscribe" => {
            let mut r = json!({ "op": "subscribe" });
            if let Some(s) = words.get(1) {
                let (w, h) = size(s)?;
                r["frame"] = json!({ "w": w, "h": h, "format": words.get(2).map(String::as_str).unwrap_or("text") });
            }
            r
        }
        other => return Err(format!("unknown request {other:?} (see caretline send --help)")),
    };
    if apply_effects && matches!(req["op"].as_str(), Some("msgs" | "keys")) {
        req["apply_effects"] = true.into();
    }
    Ok(req)
}

/// The payload `--raw` prints.
fn raw(resp: &Value) -> String {
    let r = &resp["result"];
    if let Some(f) = r["frame"].as_str() {
        return f.to_string();
    }
    if r.get("state").is_some() {
        return serde_json::to_string_pretty(&r["state"]).unwrap_or_default() + "\n";
    }
    if let Some(lines) = r["trace"].as_array() {
        return lines.iter().map(|l| l.to_string() + "\n").collect();
    }
    resp.to_string() + "\n"
}

pub fn main(argv: &[String]) -> Result<(), String> {
    let args = SendArgs::parse_from(std::iter::once("caretline send".to_string()).chain(argv.iter().cloned()));
    let socket = resolve(&args)?;
    let stream = UnixStream::connect(&socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let reader = BufReader::new(stream);
    let mut out = io::stdout().lock();

    if args.request.is_empty() {
        // Requests from stdin; print every line the server sends until it closes.
        let stdin = io::stdin();
        std::thread::spawn(move || {
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if writer.write_all(format!("{line}\n").as_bytes()).is_err() {
                    break;
                }
            }
            let _ = writer.shutdown(Shutdown::Write);
        });
        for line in reader.lines() {
            let line = line.map_err(|e| e.to_string())?;
            writeln!(out, "{line}").map_err(|e| e.to_string())?;
            out.flush().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    let req = build(&args.request, args.apply_effects)?;
    let streaming = req["op"] == "subscribe";
    writer.write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
    if !streaming {
        let _ = writer.shutdown(Shutdown::Write);
    }
    let mut failed = false;
    for line in reader.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let resp: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        failed |= resp.get("error").is_some();
        if args.raw && resp.get("result").is_some() && !streaming {
            write!(out, "{}", raw(&resp)).map_err(|e| e.to_string())?;
        } else {
            writeln!(out, "{line}").map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
    }
    if failed {
        return Err("the request failed".into());
    }
    Ok(())
}
