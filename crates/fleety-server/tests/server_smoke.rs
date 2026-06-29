use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("fleety-server-smoke-{name}-{}", std::process::id()));
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

#[test]
fn invalid_bind_exits_after_startup_setup() {
    let home = TempDir::new("home");
    let workspace = TempDir::new("workspace");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety-server"))
        .env("FLEETY_ADDR", "not-an-address")
        .env("FLEETY_AGENT_HOME", &home.0)
        .env("FLEETY_WORKSPACE", &workspace.0)
        .env_remove("FLEETY_MODEL_BASE_URL")
        .env_remove("FLEETY_MODEL")
        .output()
        .expect("run fleety-server");

    assert!(output.status.success());
    assert!(home
        .0
        .join("skills")
        .join("builtin")
        .join("fleety-use-insyra-dsl")
        .join("SKILL.md")
        .is_file());
}
