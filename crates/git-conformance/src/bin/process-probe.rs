// SPDX-FileCopyrightText: 2026 Guido <guido@ngjansen.be>
// SPDX-License-Identifier: GPL-2.0-only

//! Deliberately incompatible processes for qualification of the test runner.
use std::{
    env, fs,
    io::{self, Read, Write},
    process::{self, Command},
    thread,
    time::Duration,
};

fn main() {
    let args: Vec<_> = env::args_os().collect();
    let mode = args.get(1).and_then(|v| v.to_str()).unwrap_or("echo");
    match mode {
        "echo" => {
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input).unwrap();
            io::stdout().write_all(&input).unwrap();
        }
        "stderr" => {
            io::stderr().write_all(b"same bytes\n").unwrap();
        }
        "stdout" => {
            io::stdout().write_all(b"same bytes\n").unwrap();
        }
        "lf" => {
            io::stdout().write_all(b"line\n").unwrap();
        }
        "crlf" => {
            io::stdout().write_all(b"line\r\n").unwrap();
        }
        "exit" => process::exit(7),
        "fallback" | "missing-helper" => {
            let mut child = if mode == "fallback" {
                let mut command = Command::new(env::current_exe().unwrap());
                command.arg("stdout");
                command
            } else {
                Command::new("ferragit-deliberately-missing-helper")
            };
            process::exit(match child.status() {
                Ok(status) if status.success() => 0,
                _ => 73,
            });
        }
        "direct-exec" => {
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                let error = Command::new(&args[2]).arg("version").exec();
                eprintln!("exec denied: {}", error.kind());
                process::exit(73);
            }
            #[cfg(windows)]
            process::exit(73);
        }
        "partial-write" => {
            let mut file = fs::File::create("incomplete.lock").unwrap();
            file.write_all(b"partial\0state\n").unwrap();
            file.sync_all().unwrap();
            fs::write("ready", process::id().to_string()).unwrap();
            thread::sleep(Duration::from_secs(60));
        }
        _ => process::exit(2),
    }
}
