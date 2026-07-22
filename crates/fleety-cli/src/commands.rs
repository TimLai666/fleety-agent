//! Side-effect-free normalization of canonical commands and compatibility aliases.
//!
//! Execution still lives in the existing handlers. This module gives every
//! spelling one typed value first, so an alias cannot grow a separate network,
//! validation, or persistence path.

use clap::{Arg, ArgAction, ArgGroup, Command as ClapCommand};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Connection(Vec<String>),
    Chat(Vec<String>),
    ProviderConfig(Vec<String>),
    ModelConfig(Vec<String>),
    ModelCatalog(Vec<String>),
    ProviderAuth(Vec<String>),
    ConversationList(Vec<String>),
    ConversationResume(Vec<String>),
    Passthrough(Vec<String>),
}

impl Command {
    /// Parse only command identity and compatibility aliases. Detailed argument
    /// parsing remains in each existing handler until the full clap migration.
    pub fn normalize(args: Vec<String>) -> Self {
        let Some(root) = args.get(1).map(String::as_str) else {
            return Self::Passthrough(args);
        };
        let tail = args.get(2..).unwrap_or(&[]).to_vec();

        match root {
            "connection" | "server" => Self::Connection(tail),
            "chat" | "tui" => Self::Chat(tail),
            "auth" => Self::ProviderAuth(tail),
            "provider" => normalize_provider(tail),
            "model" => normalize_model(tail),
            "resume" => Self::ConversationResume(tail),
            "conversations" if tail.first().map(String::as_str) == Some("resume") => {
                Self::ConversationResume(tail[1..].to_vec())
            }
            "conversations" if tail.first().map(String::as_str) == Some("list") => {
                Self::ConversationList(normalize_conversation_limit(&tail[1..]))
            }
            // The old `conversations [limit]` spelling remains an alias.
            "conversations" => Self::ConversationList(tail),
            "config" => normalize_config_alias(args),
            _ => Self::Passthrough(args),
        }
    }

    /// Convert the canonical value to the one existing execution spelling.
    /// There is deliberately one spelling per handler after this point.
    pub fn into_dispatch_args(self) -> Vec<String> {
        match self {
            Self::Connection(tail) => with_root("server", tail),
            Self::Chat(tail) => with_root("tui", tail),
            Self::ProviderConfig(tail) => config_args("provider", tail),
            Self::ModelConfig(tail) => config_args("model", tail),
            Self::ModelCatalog(tail) => with_root("model-catalog", tail),
            Self::ProviderAuth(tail) => with_root("auth", tail),
            Self::ConversationList(tail) => with_root("conversations", tail),
            Self::ConversationResume(tail) => with_root("resume", tail),
            Self::Passthrough(args) => args,
        }
    }
}

fn normalize_conversation_limit(tail: &[String]) -> Vec<String> {
    match tail {
        [flag, value] if flag == "--limit" => vec![value.clone()],
        _ => tail.to_vec(),
    }
}

/// The complete side-effect-free command inventory. Parsing this tree is the
/// first operation in `main`, so generated help and usage errors cannot seed
/// config, migrate files, initialize logging, or connect to an owner.
pub fn validate(args: &[String]) -> Result<(), clap::Error> {
    command().try_get_matches_from(args).map(|_| ())
}

/// Treat a trailing `help` like `--help` only when the resolved target is a
/// leaf with no positional arguments. This keeps the friendly
/// `fleety status help` spelling without stealing legitimate values such as
/// `fleety ask help` or a profile literally named `help`.
pub fn normalize_trailing_help(mut args: Vec<String>) -> Vec<String> {
    if args.len() < 3 || args.last().map(String::as_str) != Some("help") {
        return args;
    }

    let tree = command();
    let mut node = &tree;
    let mut index = 1;
    let end = args.len() - 1;
    while index < end {
        let token = args[index].as_str();
        if token == "--" {
            return args;
        }
        if let Some(child) = node.find_subcommand(token) {
            node = child;
            index += 1;
            continue;
        }
        if let Some(option) = token.strip_prefix("--") {
            let name = option.split_once('=').map_or(option, |(name, _)| name);
            let Some(arg) = node
                .get_arguments()
                .find(|arg| arg.get_long() == Some(name))
            else {
                return args;
            };
            index += 1;
            if !option.contains('=') && arg.get_action().takes_values() {
                index += 1;
            }
            continue;
        }
        if let Some(shorts) = token.strip_prefix('-').filter(|value| !value.is_empty()) {
            let Some(short) = shorts.chars().next() else {
                return args;
            };
            let Some(arg) = node
                .get_arguments()
                .find(|arg| arg.get_short() == Some(short))
            else {
                return args;
            };
            index += 1;
            let attached = &shorts[short.len_utf8()..];
            if attached.is_empty() && arg.get_action().takes_values() {
                index += 1;
            }
            continue;
        }
        return args;
    }

    if node.get_subcommands().next().is_none() && node.get_positionals().next().is_none() {
        if let Some(last) = args.last_mut() {
            *last = "--help".to_string();
        }
    }
    args
}

pub fn render_error(error: clap::Error) -> std::process::ExitCode {
    let code = error.exit_code();
    if code == 0 {
        let _ = error.print();
    } else {
        let rendered = error.to_string();
        eprint!("{}", crate::terminal_safe_multiline(&rendered));
        if !rendered.to_ascii_lowercase().contains("usage:") {
            eprintln!("\n{}", command().render_usage());
        }
    }
    std::process::ExitCode::from(code as u8)
}

pub fn command() -> ClapCommand {
    ClapCommand::new("fleety")
        .bin_name("fleety")
        .version(agent_core::VERSION)
        .about("The Fleety CLI")
        .subcommand_required(false)
        .arg(
            Arg::new("legacy-version")
                .short('v')
                .action(ArgAction::Version)
                .hide(true),
        )
        .arg(
            Arg::new("profile")
                .long("profile")
                .value_name("PROFILE")
                .help("Use a saved profile for this invocation without making it current")
                .global(true),
        )
        .arg(url_override("server", 's'))
        .arg(
            Arg::new("url")
                .long("url")
                .value_name("WS_URL")
                .hide(true)
                .global(true),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .help("Emit one stable JSON envelope")
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("quiet")
                .long("quiet")
                .help("Suppress context and other non-result prose")
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("no-color")
                .long("no-color")
                .help("Disable ANSI color output")
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("warnings")
                .long("warnings")
                .help("Show compatibility and deprecation warnings")
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .subcommands([
            leaf("init", "Connect and save a Server profile")
                .arg(positional("ws_url", false))
                .arg(option("name"))
                .arg(option("pairing-code")),
            ask_command(),
            leaf("chat", "Open the terminal workspace").visible_alias("tui"),
            conversations_command(),
            leaf("resume", "Compatibility alias for conversations resume")
                .hide(true)
                .arg(positional("conversation_id", true))
                .arg(positional("after_seq", false).value_parser(clap::value_parser!(u64))),
            connection_command(),
            provider_command("provider"),
            model_command("model"),
            config_command(),
            leaf("status", "Show CLI, Daemon, and Server status"),
            leaf("doctor", "Run bounded read-only diagnostics"),
            completion_command(),
            leaf("voice", "Start a voice conversation"),
            audit_command(),
            rollback_command(),
            leaf("pair", "Enroll this device").arg(positional("code", true)),
            leaf("pair-code", "Mint a pairing code"),
            daemon_command(),
            leaf("update", "Update Fleety components"),
            acp_command(),
            leaf("version", "Print the version"),
            auth_compat_command(),
        ])
}

fn leaf(name: &'static str, about: &'static str) -> ClapCommand {
    ClapCommand::new(name).about(about)
}

fn positional(name: &'static str, required: bool) -> Arg {
    Arg::new(name).required(required)
}

fn option(name: &'static str) -> Arg {
    Arg::new(name).long(name).value_name("VALUE")
}

fn url_override(name: &'static str, short: char) -> Arg {
    Arg::new(name)
        .short(short)
        .long(name)
        .value_name("WS_URL")
        .help("Connect to a raw WebSocket URL for this invocation (legacy)")
        .global(true)
}

fn ask_command() -> ClapCommand {
    leaf("ask", "Send one prompt")
        .group(
            ArgGroup::new("input")
                .args(["text", "image", "audio", "video", "file"])
                .required(true)
                .multiple(true),
        )
        .arg(
            Arg::new("text")
                .value_name("TEXT")
                .num_args(0..)
                .action(ArgAction::Append),
        )
        .arg(
            Arg::new("image")
                .short('i')
                .long("image")
                .value_name("PATH")
                .action(ArgAction::Append),
        )
        .arg(option("audio").action(ArgAction::Append))
        .arg(option("video").action(ArgAction::Append))
        .arg(option("file").action(ArgAction::Append))
}

fn conversations_command() -> ClapCommand {
    leaf("conversations", "List or resume conversations")
        .arg(positional("legacy_limit", false).value_parser(clap::value_parser!(u32)))
        .subcommands([
            leaf("list", "List recent conversations")
                .group(ArgGroup::new("list_limit").args(["limit", "legacy_list_limit"]))
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u32)),
                )
                .arg(
                    positional("legacy_list_limit", false)
                        .value_parser(clap::value_parser!(u32))
                        .hide(true),
                ),
            leaf("resume", "Resume a conversation")
                .arg(positional("conversation_id", true))
                .arg(positional("after_seq", false).value_parser(clap::value_parser!(u64))),
        ])
}

fn connection_command() -> ClapCommand {
    leaf("connection", "Manage named Server profiles")
        .visible_alias("server")
        .subcommand_required(true)
        .subcommands([
            leaf("add", "Add a profile")
                .arg(positional("name", true))
                .arg(positional("ws_url", true))
                .arg(option("label"))
                .arg(Arg::new("use").long("use").action(ArgAction::SetTrue)),
            leaf("use", "Select the current profile").arg(positional("name", true)),
            leaf("list", "List profiles"),
            leaf("show", "Show one profile").arg(positional("name", false)),
            leaf("current", "Show the current profile"),
            leaf("rename", "Rename a profile")
                .arg(positional("old", true))
                .arg(positional("new", true)),
            leaf("remove", "Remove a profile")
                .arg(positional("name", true))
                .arg(Arg::new("force").long("force").action(ArgAction::SetTrue)),
            leaf("set-url", "Change a profile URL")
                .arg(positional("name", true))
                .arg(positional("ws_url", true)),
        ])
}

fn provider_command(name: &'static str) -> ClapCommand {
    leaf(name, "Manage Server-owned providers and OAuth")
        .subcommand_required(true)
        .subcommands([
            provider_fields(
                leaf("add", "Add a provider").arg(positional("name", true)),
                true,
            ),
            provider_fields(
                leaf("set", "Update a provider").arg(positional("name", true)),
                false,
            )
            .arg(
                Arg::new("clear-key")
                    .long("clear-key")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("key"),
            )
            .hide(true),
            leaf("edit", "Open the provider editor"),
            leaf("remove", "Remove a provider").arg(positional("name", true)),
            leaf("list", "List providers"),
            leaf("login", "Sign in an OAuth provider")
                .arg(positional("provider", true))
                .arg(
                    Arg::new("no-browser")
                        .long("no-browser")
                        .action(ArgAction::SetTrue),
                ),
            leaf("logout", "Sign out an OAuth provider").arg(positional("provider", true)),
            leaf("status", "Show OAuth status").arg(positional("provider", false)),
        ])
}

fn provider_fields(command: ClapCommand, kind_required: bool) -> ClapCommand {
    command
        .arg(option("type").required(kind_required))
        .arg(option("base-url"))
        .arg(option("key"))
}

fn model_command(name: &'static str) -> ClapCommand {
    leaf(name, "Manage Server-owned model roles")
        .subcommand_required(true)
        .subcommands([
            leaf("catalog", "Fetch a Provider's available model IDs")
                .arg(positional("provider", true))
                .arg(option("role")),
            leaf("list", "List model roles"),
            leaf("show", "Show a model role").arg(positional("role", false)),
            leaf("unset", "Unset a model role").arg(positional("role", true)),
            leaf("set", "Set a model role")
                .arg(positional("role", true))
                .arg(option("member").required(true).action(ArgAction::Append))
                .arg(option("strategy"))
                .arg(Arg::new("stream").long("stream").action(ArgAction::Count))
                .arg(option("modalities").action(ArgAction::Append))
                .arg(option("effort").action(ArgAction::Append)),
        ])
}

fn config_command() -> ClapCommand {
    fleety_tools::config::clap_command_for_cli().arg(
        Arg::new("owner")
            .long("owner")
            .visible_alias("target")
            .value_name("server|daemon|cli|DEVICE_ID")
            .help("Route this config operation to its actual owner")
            .global(true),
    )
}

fn audit_command() -> ClapCommand {
    leaf("audit", "Inspect device audit entries")
        .subcommand_required(true)
        .subcommands([
            leaf("list", "List audit entries")
                .arg(positional("limit", false).value_parser(clap::value_parser!(u32))),
            leaf("show", "Show one audit entry")
                .arg(positional("index", true).value_parser(clap::value_parser!(u64))),
        ])
}

fn rollback_command() -> ClapCommand {
    leaf("rollback", "Inspect or apply backups")
        .subcommand_required(true)
        .subcommands([
            leaf("list", "List backups"),
            leaf("apply", "Restore a backup").arg(positional("backup_id", true)),
        ])
}

fn daemon_command() -> ClapCommand {
    leaf("daemon", "Manage the local Daemon")
        .subcommand_required(true)
        .subcommands(
            [
                "install",
                "uninstall",
                "start",
                "stop",
                "restart",
                "enable",
                "disable",
                "status",
                "up",
                "down",
                "update",
            ]
            .into_iter()
            .map(|name| leaf(name, "Daemon lifecycle action")),
        )
}

fn completion_command() -> ClapCommand {
    leaf("completion", "Generate shell completion source")
        .after_help(
            "Examples:\n  fleety completion bash > ~/.local/share/bash-completion/completions/fleety\n  fleety completion powershell | Out-String | Invoke-Expression",
        )
        .arg(
            positional("shell", true).value_parser([
                "bash",
                "zsh",
                "fish",
                "powershell",
                "elvish",
            ]),
        )
}

fn acp_command() -> ClapCommand {
    leaf("acp", "Run as an ACP agent").subcommand(
        leaf("install", "Print or install ACP editor configuration")
            .arg(positional("editor", false))
            .arg(option("server")),
    )
}

fn auth_compat_command() -> ClapCommand {
    leaf("auth", "Compatibility alias for provider OAuth")
        .hide(true)
        .subcommand_required(true)
        .subcommands([
            leaf("login", "Sign in")
                .arg(positional("provider", true))
                .arg(
                    Arg::new("no-browser")
                        .long("no-browser")
                        .action(ArgAction::SetTrue),
                ),
            leaf("logout", "Sign out").arg(positional("provider", true)),
            leaf("status", "Show status").arg(positional("provider", false)),
        ])
}

fn normalize_config_alias(mut args: Vec<String>) -> Command {
    for arg in &mut args {
        if arg == "--owner" {
            *arg = "--target".to_string();
        }
    }
    // `--target` can occur anywhere in the legacy config syntax. Locate the
    // first non-option command while skipping its value, then remove only that
    // command word. The remaining context is preserved for owner resolution.
    let mut index = 2;
    while index < args.len() {
        if matches!(args[index].as_str(), "--target" | "--owner") {
            index += 2;
            continue;
        }
        break;
    }

    match args.get(index).map(String::as_str) {
        Some("provider") => {
            let mut tail = args[2..].to_vec();
            tail.remove(index - 2);
            normalize_provider(tail)
        }
        Some("model") => {
            let mut tail = args[2..].to_vec();
            tail.remove(index - 2);
            normalize_model(tail)
        }
        _ => Command::Passthrough(args),
    }
}

fn normalize_model(tail: Vec<String>) -> Command {
    let mut action_tail = Vec::with_capacity(tail.len());
    let mut index = 0;
    while index < tail.len() {
        if tail[index] == "--target" {
            let Some(value) = tail.get(index + 1) else {
                return Command::ModelConfig(tail);
            };
            if value != "server" {
                return Command::ModelConfig(tail);
            }
            index += 2;
            continue;
        }
        action_tail.push(tail[index].clone());
        index += 1;
    }
    if action_tail.first().map(String::as_str) == Some("catalog") {
        Command::ModelCatalog(action_tail[1..].to_vec())
    } else {
        Command::ModelConfig(tail)
    }
}

fn normalize_provider(tail: Vec<String>) -> Command {
    let mut action = None;
    let mut auth_tail = Vec::with_capacity(tail.len());
    let mut index = 0;
    while index < tail.len() {
        if tail[index] == "--target" {
            let Some(value) = tail.get(index + 1) else {
                return Command::ProviderConfig(tail);
            };
            if value != "server" {
                return Command::ProviderConfig(tail);
            }
            index += 2;
            continue;
        }
        if action.is_none() {
            action = Some(tail[index].as_str());
        }
        auth_tail.push(tail[index].clone());
        index += 1;
    }

    if matches!(action, Some("login" | "logout" | "status")) {
        Command::ProviderAuth(auth_tail)
    } else {
        Command::ProviderConfig(tail)
    }
}

fn with_root(root: &str, tail: Vec<String>) -> Vec<String> {
    let mut args = Vec::with_capacity(tail.len() + 2);
    args.push("fleety".to_string());
    args.push(root.to_string());
    args.extend(tail);
    args
}

fn config_args(kind: &str, mut tail: Vec<String>) -> Vec<String> {
    for arg in &mut tail {
        if arg == "--owner" {
            *arg = "--target".to_string();
        }
    }
    let mut args = Vec::with_capacity(tail.len() + 3);
    args.push("fleety".to_string());
    args.push("config".to_string());

    // Canonical top-level provider/model commands are always Server-owned.
    // A legacy config alias may already carry explicit owner context.
    if !tail
        .iter()
        .any(|arg| matches!(arg.as_str(), "--target" | "--owner"))
    {
        args.extend(["--target".to_string(), "server".to_string()]);
    }
    args.push(kind.to_string());
    args.append(&mut tail);
    args
}

#[cfg(test)]
mod tests {
    use super::{command, normalize_trailing_help, Command};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn compatibility_spellings_become_the_same_typed_value() {
        assert_eq!(
            Command::normalize(args(&["fleety", "connection", "list"])),
            Command::normalize(args(&["fleety", "server", "list"]))
        );
        assert_eq!(
            Command::normalize(args(&["fleety", "chat"])),
            Command::normalize(args(&["fleety", "tui"]))
        );
        assert_eq!(
            Command::normalize(args(&["fleety", "provider", "login", "codex"])),
            Command::normalize(args(&["fleety", "auth", "login", "codex"]))
        );
        assert_eq!(
            Command::normalize(args(&["fleety", "conversations", "list", "20"])),
            Command::normalize(args(&["fleety", "conversations", "20"]))
        );
        assert_eq!(
            Command::normalize(args(&["fleety", "conversations", "list", "--limit", "20",])),
            Command::normalize(args(&["fleety", "conversations", "20"]))
        );
        assert_eq!(
            Command::normalize(args(&["fleety", "conversations", "resume", "c1", "2"])),
            Command::normalize(args(&["fleety", "resume", "c1", "2"]))
        );
    }

    #[test]
    fn documented_conversation_limit_flag_is_accepted_and_normalized() {
        let canonical = args(&["fleety", "conversations", "list", "--limit", "10"]);
        command()
            .try_get_matches_from(canonical.clone())
            .expect("documented --limit spelling");
        assert_eq!(
            Command::normalize(canonical).into_dispatch_args(),
            args(&["fleety", "conversations", "10"])
        );
    }

    #[test]
    fn config_nested_provider_and_model_use_canonical_variants() {
        assert_eq!(
            Command::normalize(args(&["fleety", "config", "provider", "list"])),
            Command::ProviderConfig(args(&["list"]))
        );
        assert_eq!(
            Command::normalize(args(&["fleety", "provider", "list"])),
            Command::normalize(args(&["fleety", "config", "provider", "list"]))
        );
        assert_eq!(
            Command::normalize(args(&[
                "fleety", "config", "--target", "server", "model", "list"
            ])),
            Command::ModelConfig(args(&["--target", "server", "list"]))
        );
        assert_eq!(
            Command::normalize(args(&["fleety", "model", "list"])),
            Command::normalize(args(&["fleety", "config", "model", "list"]))
        );
    }

    #[test]
    fn model_catalog_has_one_typed_request() {
        let canonical = Command::normalize(args(&[
            "fleety", "model", "catalog", "codex", "--role", "main",
        ]));
        let compatibility = Command::normalize(args(&[
            "fleety", "config", "--target", "server", "model", "catalog", "codex", "--role", "main",
        ]));
        assert_eq!(canonical, compatibility);
        assert_eq!(
            canonical.into_dispatch_args(),
            args(&["fleety", "model-catalog", "codex", "--role", "main"])
        );
    }

    #[test]
    fn config_nested_oauth_actions_use_the_provider_auth_value() {
        for action in ["login", "logout", "status"] {
            assert_eq!(
                Command::normalize(args(&["fleety", "provider", action, "codex"])),
                Command::normalize(args(&["fleety", "config", "provider", action, "codex"]))
            );
            assert_eq!(
                Command::normalize(args(&["fleety", "provider", action, "codex"])),
                Command::normalize(args(&[
                    "fleety", "config", "--target", "server", "provider", action, "codex"
                ]))
            );
        }
    }

    #[test]
    fn trailing_help_word_is_help_only_for_non_positional_leaves() {
        for values in [
            &["fleety", "status", "help"][..],
            &["fleety", "connection", "list", "help"],
            &["fleety", "server", "current", "help"],
            &["fleety", "config", "--owner", "server", "list", "help"],
            &["fleety", "daemon", "status", "help"],
            &["fleety", "-s", "ws://host.test:8787", "status", "help"],
            &["fleety", "-s=ws://host.test:8787", "status", "help"],
            &["fleety", "-sws://host.test:8787", "status", "help"],
        ] {
            let normalized = normalize_trailing_help(args(values));
            assert_eq!(normalized.last().map(String::as_str), Some("--help"));
            command()
                .try_get_matches_from(normalized)
                .expect_err("help exits through clap's display-help path");
        }

        for values in [
            &["fleety", "ask", "help"][..],
            &["fleety", "pair", "help"],
            &["fleety", "connection", "show", "help"],
            &["fleety", "provider", "status", "help"],
        ] {
            assert_eq!(normalize_trailing_help(args(values)), args(values));
        }
    }
}
