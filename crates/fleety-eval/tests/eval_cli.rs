use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("fleety-eval-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fleety-eval"))
        .args(args)
        .output()
        .expect("run fleety-eval")
}

#[test]
fn no_args_exits_with_usage_code() {
    let output = run(&[]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: fleety-eval"));
}

#[test]
fn run_empty_dir_reports_no_goldens() {
    let temp = TempDir::new("empty");
    let output = run(&["run", temp.0.to_str().expect("utf8 temp")]);

    assert_eq!(output.status.code(), Some(255));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no .jsonl files found"));
}

#[test]
fn run_workspace_goldens_passes() {
    let goldens = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("goldens");
    let output = run(&["run", goldens.to_str().expect("utf8 goldens")]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary = stdout.lines().last().expect("summary line");
    let fields = summary.split_whitespace().collect::<Vec<_>>();
    assert_eq!(fields.len(), 6, "unexpected summary: {summary}");
    let passed = fields[0].parse::<usize>().expect("passed count");
    let failed = fields[2].parse::<usize>().expect("failed count");
    let total = fields[4].parse::<usize>().expect("total count");
    assert!(passed > 0);
    assert_eq!(failed, 0);
    assert_eq!(passed, total);
}
