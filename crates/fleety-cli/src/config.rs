//! `fleety config` — inspect and edit settings from the terminal.
//!
//! Backed by the shared typed registry in `fleety_tools::config`. `list/get`
//! show the resolved value and its source (env / config / default), secrets
//! masked; `set/unset` edit `~/.fleety/config.toml` after validating the key;
//! `edit` is a small interactive loop. Read precedence stays env → config →
//! default, so an explicit env var always wins.

use agent_core::{CoreError, Result};
use fleety_tools::config::{self, ConfigMap, Source};

/// A parsed `config` subcommand (pure; the executor does the I/O).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigCommand {
    List,
    Get(String),
    Set(String, String),
    Unset(String),
    Edit,
    Help,
}

/// Parse `config <args...>` into a command. Pure and unit-testable.
pub fn parse(args: &[String]) -> ConfigCommand {
    match args.first().map(String::as_str) {
        Some("list") | None => ConfigCommand::List,
        Some("get") => args
            .get(1)
            .map(|k| ConfigCommand::Get(k.clone()))
            .unwrap_or(ConfigCommand::Help),
        Some("set") => match (args.get(1), args.get(2)) {
            (Some(k), Some(v)) => ConfigCommand::Set(k.clone(), v.clone()),
            _ => ConfigCommand::Help,
        },
        Some("unset") => args
            .get(1)
            .map(|k| ConfigCommand::Unset(k.clone()))
            .unwrap_or(ConfigCommand::Help),
        Some("edit") => ConfigCommand::Edit,
        _ => ConfigCommand::Help,
    }
}

/// One display row for `list` (key, scope, shown value, source label).
fn rows(map: &ConfigMap) -> Vec<(String, String, String, String)> {
    let mut out = Vec::new();
    for s in config::registry() {
        let Some(resolved) = config::resolve(s.key, map) else {
            continue;
        };
        let source = match resolved.source {
            Source::Env => "env",
            Source::Config => "config",
            Source::Default => "default",
        };
        out.push((
            s.key.to_string(),
            s.scope.as_str().to_string(),
            config::display_value(s, &resolved.value),
            source.to_string(),
        ));
    }
    out
}

/// Run a `config` subcommand.
pub fn run(args: &[String]) -> Result<()> {
    let path = config::config_path();
    match parse(args) {
        ConfigCommand::List => {
            let map = config::load(&path);
            println!("Settings (env → config → default; secrets masked):\n");
            for (key, scope, value, source) in rows(&map) {
                println!("  [{scope:6}] {key:<26} = {value}  ({source})");
            }
            println!(
                "\nEdit with: fleety config set <KEY> <VALUE>   (file: {})",
                path.display()
            );
            Ok(())
        }
        ConfigCommand::Get(key) => {
            let setting = config::find(&key)
                .ok_or_else(|| CoreError::Message(format!("unknown setting '{key}'")))?;
            let map = config::load(&path);
            let Some(r) = config::resolve(&key, &map) else {
                return Ok(());
            };
            let src = match r.source {
                Source::Env => "env",
                Source::Config => "config",
                Source::Default => "default",
            };
            println!(
                "{} = {}  ({src})",
                key,
                config::display_value(setting, &r.value)
            );
            Ok(())
        }
        ConfigCommand::Set(key, value) => {
            let setting = config::find(&key).ok_or_else(|| {
                CoreError::Message(format!(
                    "unknown setting '{key}'. Run `fleety config list` to see valid keys."
                ))
            })?;
            let mut map = config::load(&path);
            map.insert((setting.scope, key.clone()), value);
            config::save(&path, &map)?;
            println!("set {key} (scope {})", setting.scope.as_str());
            Ok(())
        }
        ConfigCommand::Unset(key) => {
            let setting = config::find(&key)
                .ok_or_else(|| CoreError::Message(format!("unknown setting '{key}'")))?;
            let mut map = config::load(&path);
            map.remove(&(setting.scope, key.clone()));
            config::save(&path, &map)?;
            println!("unset {key} (reverts to env/default)");
            Ok(())
        }
        ConfigCommand::Edit => edit_interactive(&path),
        ConfigCommand::Help => {
            println!(
                "usage: fleety config [list | get <KEY> | set <KEY> <VALUE> | unset <KEY> | edit]"
            );
            Ok(())
        }
    }
}

/// A small interactive editor: list settings, pick one by number, type a new
/// value, save. (Line-based; a richer ratatui screen is a follow-up.)
fn edit_interactive(path: &std::path::Path) -> Result<()> {
    use std::io::Write;
    let mut map = config::load(path);
    loop {
        let listing = rows(&map);
        println!("\nSettings (enter a number to edit, blank to finish):");
        for (i, (key, scope, value, source)) in listing.iter().enumerate() {
            println!("  {i:>2}) [{scope:6}] {key:<26} = {value}  ({source})");
        }
        print!("> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let Ok(idx) = line.parse::<usize>() else {
            println!("not a number");
            continue;
        };
        let Some(setting) = config::registry().get(idx) else {
            println!("out of range");
            continue;
        };
        print!("new value for {} (blank to cancel): ", setting.key);
        std::io::stdout().flush().ok();
        let mut val = String::new();
        if std::io::stdin().read_line(&mut val).is_err() {
            break;
        }
        let val = val.trim().to_string();
        if val.is_empty() {
            continue;
        }
        map.insert((setting.scope, setting.key.to_string()), val);
        config::save(path, &map)?;
        println!("saved {}", setting.key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_commands() {
        assert_eq!(parse(&v(&[])), ConfigCommand::List);
        assert_eq!(parse(&v(&["list"])), ConfigCommand::List);
        assert_eq!(
            parse(&v(&["get", "FLEETY_ADDR"])),
            ConfigCommand::Get("FLEETY_ADDR".into())
        );
        assert_eq!(
            parse(&v(&["set", "FLEETY_ADDR", "0.0.0.0:9000"])),
            ConfigCommand::Set("FLEETY_ADDR".into(), "0.0.0.0:9000".into())
        );
        assert_eq!(
            parse(&v(&["unset", "FLEETY_ADDR"])),
            ConfigCommand::Unset("FLEETY_ADDR".into())
        );
        assert_eq!(parse(&v(&["edit"])), ConfigCommand::Edit);
        // Missing operands → Help.
        assert_eq!(parse(&v(&["get"])), ConfigCommand::Help);
        assert_eq!(parse(&v(&["set", "X"])), ConfigCommand::Help);
    }

    #[test]
    fn rows_cover_registry_with_sources() {
        let map = ConfigMap::new();
        let r = rows(&map);
        assert_eq!(r.len(), config::registry().len());
        // With an empty config and (typically) unset env, sources are default.
        assert!(r
            .iter()
            .any(|(k, _, _, src)| k == "FLEETY_POLICY" && src == "default"));
        // Secret masked in the row.
        let mut m = ConfigMap::new();
        m.insert(
            (config::Scope::Server, "FLEETY_TOKEN".into()),
            "secret".into(),
        );
        let r = rows(&m);
        let tok = r.iter().find(|(k, ..)| k == "FLEETY_TOKEN").unwrap();
        assert_eq!(tok.2, "********");
    }
}
