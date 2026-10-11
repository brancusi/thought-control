//! keydump: what the terminal sends for each key, byte for byte. The e2e harness runs it in the
//! real terminal (as `keydump`, and as `thc`, so thc's WezTerm forwards treat it as thc) with the
//! kitty keyboard flags a program asks for, presses keys through WezTerm and reads the log.
//!
//!     keydump FLAGS LOG [fictty]   FLAGS: the kitty flags to push (0: none; thc pushes 1, fictty 1|4 = 5);
//!                                  `fictty` sets the user var a fictty screen sets
//!
//! Each read is one line of the log: the bytes, escaped (`\x1b[99;9u`). SIGTERM ends it.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn stty(args: &[&str]) {
    let _ = Command::new("stty").args(args).stdin(Stdio::inherit()).status();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flags: u32 = args.get(1).and_then(|f| f.parse().ok()).unwrap_or(0);
    let log = args.get(2).cloned().unwrap_or_else(|| "keydump.log".into());
    stty(&["raw", "-echo"]);
    let mut out = std::io::stdout();
    if flags > 0 {
        let _ = write!(out, "\x1b[>{flags}u");
    }
    // `keydump FLAGS LOG fictty`: say what a fictty screen says (the `fictty` user var, OSC 1337),
    // so fictty's WezTerm module forwards its ⌘ keys here.
    if args.get(3).is_some_and(|a| a == "fictty") {
        let _ = write!(out, "\x1b]1337;SetUserVar=fictty=MQ==\x07");
    }
    // Ask which flags are on: the answer (`CSI ? flags u`) is the log's first line.
    let _ = write!(out, "\x1b[?u");
    // Mouse reports too (SGR), as thc and fictty ask for them.
    let _ = write!(out, "\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h\r\nkeydump ready (flags {flags})\r\n");
    let _ = out.flush();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&log).expect("log");
    let mut buf = [0u8; 256];
    let mut stdin = std::io::stdin();
    while let Ok(n) = stdin.read(&mut buf) {
        if n == 0 {
            break;
        }
        let esc: String = buf[..n].iter().map(|&b| if (0x20..0x7f).contains(&b) && b != b'\\' { (b as char).to_string() } else { format!("\\x{b:02x}") }).collect();
        let _ = writeln!(f, "{esc}");
        let _ = f.flush();
        // ⌃] ends it from the keyboard.
        if buf[..n] == [0x1d] {
            break;
        }
    }
    let _ = write!(out, "\x1b[<u\x1b[?1000l\x1b[?1002l\x1b[?1006l\x1b[?2004l");
    stty(&["sane"]);
}
