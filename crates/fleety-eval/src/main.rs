//! `fleety-eval` CLI: run JSONL golden files and report pass/fail.
//!
//! Usage:
//!   fleety-eval run <path> [<path> …]
//!
//! Each `<path>` may be a single `.jsonl` file or a directory (in which case
//! all `*.jsonl` files under it are picked up, recursively). Exit code is the
//! number of failed goldens (capped at 255), so CI can gate on `!= 0`.

#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fleety_eval::{run_file, Verdict};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let mut args = std::env::args().skip(1);
    let cmd = args.next();
    let paths: Vec<String> = args.collect();
    match cmd.as_deref() {
        Some("run") if !paths.is_empty() => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("cannot start tokio runtime: {e}");
                    return ExitCode::from(255);
                }
            };
            runtime.block_on(async move {
                let files = collect_files(&paths);
                if files.is_empty() {
                    eprintln!("no .jsonl files found at: {:?}", paths);
                    return ExitCode::from(255);
                }
                let mut all = Vec::new();
                for file in &files {
                    match run_file(file).await {
                        Ok(mut v) => all.append(&mut v),
                        Err(e) => {
                            eprintln!("{}: {}", file.display(), e);
                            return ExitCode::from(255);
                        }
                    }
                }
                report(&all);
                let failed = all.iter().filter(|v| !v.passed).count();
                ExitCode::from(failed.min(255) as u8)
            })
        }
        _ => {
            eprintln!("usage: fleety-eval run <path.jsonl|dir> [<path> …]");
            ExitCode::from(2)
        }
    }
}

/// Expand each input into the list of `.jsonl` files to run.
fn collect_files(inputs: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for input in inputs {
        let path = PathBuf::from(input);
        if path.is_dir() {
            walk_jsonl(&path, &mut out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn walk_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_jsonl(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn report(verdicts: &[Verdict]) {
    let passed = verdicts.iter().filter(|v| v.passed).count();
    let failed = verdicts.len() - passed;
    for v in verdicts {
        if v.passed {
            println!(
                "PASS  {}  ({} step{}, tools: {})",
                v.name,
                v.steps,
                if v.steps == 1 { "" } else { "s" },
                if v.tools_called.is_empty() {
                    "—".to_string()
                } else {
                    v.tools_called.join(", ")
                },
            );
        } else {
            println!("FAIL  {}", v.name);
            for f in &v.failures {
                for line in f.lines() {
                    println!("        {line}");
                }
            }
        }
    }
    println!(
        "\n{passed} passed, {failed} failed, {} total",
        verdicts.len()
    );
}
