//! Native computer-use (desktop control), shared so the same tools run on the
//! server and on any device via `fleetyd` —
//! `device_exec(device="laptop", tool="computer_screenshot")` drives the
//! laptop's screen. Input via `enigo`, screen capture via `xcap`; both are
//! synchronous and touch platform input/capture APIs, so every call runs on a
//! blocking thread and builds its own `Enigo` (it isn't `Send`).
//!
//! Needs a real desktop session. Headless hosts, or Linux under Wayland (where
//! synthetic input is restricted), surface an actionable error rather than
//! crashing. This is the **most intrusive** interface — `policy.md` ranks it
//! last (prefer a dedicated API/MCP > browser CDP > computer-use); screenshots
//! are low-impact (Read), every input action is Mutate.

use async_trait::async_trait;
use base64::Engine;
use enigo::{
    Axis, Button, Coordinate,
    Direction::{Click, Press, Release},
    Enigo, Key, Keyboard, Mouse, Settings,
};
use serde_json::{json, Value};
use xcap::Monitor;

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Register the native computer-use tools.
pub fn register_computer(registry: &mut ToolRegistry) {
    registry.register(Box::new(ComputerScreenshot));
    registry.register(Box::new(ComputerMove));
    registry.register(Box::new(ComputerClick));
    registry.register(Box::new(ComputerType));
    registry.register(Box::new(ComputerKey));
    registry.register(Box::new(ComputerScroll));
}

/// Why desktop control may be unavailable, and what to do — shared by the input
/// and capture paths so the agent always gets the same actionable guidance.
/// Notably covers the background-service case: a Windows service runs in session
/// 0 with no interactive desktop, so a user must be logged in for these tools.
pub(crate) fn headless_remediation() -> &'static str {
    "this device needs an interactive desktop session: headless hosts, \
     Wayland-restricted input, or a background service running in Windows session 0 \
     (no desktop) can't be driven this way — log in to an interactive session and retry"
}

fn enigo() -> Result<Enigo> {
    Enigo::new(&Settings::default()).map_err(|e| {
        CoreError::Message(format!(
            "cannot access the desktop for input ({e}); {}",
            headless_remediation()
        ))
    })
}

/// Run a closure that needs `Enigo`, on a blocking thread (enigo isn't `Send`).
async fn with_enigo<F>(f: F) -> Result<Value>
where
    F: FnOnce(&mut Enigo) -> Result<Value> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut e = enigo()?;
        f(&mut e)
    })
    .await
    .map_err(|e| CoreError::Message(format!("computer-use task join error: {e}")))?
}

fn map_input<T>(r: std::result::Result<T, enigo::InputError>) -> Result<T> {
    r.map_err(|e| CoreError::Message(format!("input failed: {e}")))
}

fn arg_i32(args: &Value, key: &str) -> Result<i32> {
    args.get(key)
        .and_then(Value::as_i64)
        .map(|n| n as i32)
        .ok_or_else(|| CoreError::Message(format!("missing required integer argument '{key}'")))
}

/// Map a key name (or single character) to an enigo `Key`.
fn parse_key(name: &str) -> Result<Key> {
    let k = match name.to_ascii_lowercase().as_str() {
        "return" | "enter" => Key::Return,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "space" => Key::Space,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Key::Unicode(c),
                _ => {
                    return Err(CoreError::Message(format!(
                        "unknown key '{name}'; use a single character or a named key (enter, tab, esc, up, f5, …)"
                    )))
                }
            }
        }
    };
    Ok(k)
}

fn modifier_key(name: &str) -> Result<Key> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Key::Control,
        "alt" | "option" => Key::Alt,
        "shift" => Key::Shift,
        "meta" | "cmd" | "command" | "win" | "super" => Key::Meta,
        other => return Err(CoreError::Message(format!("unknown modifier '{other}'"))),
    })
}

struct ComputerScreenshot;

#[async_trait]
impl Tool for ComputerScreenshot {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "computer_screenshot".to_string(),
            description: "Capture the desktop as a base64 PNG (low-impact way to see a device's \
                 screen). Runs on the device you target (bare = server; via device_exec = that \
                 device). Pass `monitor` (0-based) to pick a screen."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "monitor": { "type": "integer", "description": "0-based monitor index (default 0)" } }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let idx = args.get("monitor").and_then(Value::as_u64).unwrap_or(0) as usize;
        tokio::task::spawn_blocking(move || {
            let monitors = Monitor::all().map_err(|e| {
                CoreError::Message(format!(
                    "cannot enumerate monitors ({e}); {}",
                    headless_remediation()
                ))
            })?;
            let monitor = monitors
                .get(idx)
                .or_else(|| monitors.first())
                .ok_or_else(|| CoreError::Message("no monitors found".to_string()))?;
            let image = monitor
                .capture_image()
                .map_err(|e| CoreError::Message(format!("screen capture failed: {e}")))?;
            let (w, h) = (image.width(), image.height());
            let raw = image.into_raw();
            let png = encode_png(w, h, &raw)?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
            Ok(json!({ "image_base64": b64, "format": "png", "width": w, "height": h, "monitor": idx }))
        })
        .await
        .map_err(|e| CoreError::Message(format!("screenshot task join error: {e}")))?
    }
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .map_err(|e| CoreError::Message(format!("png header: {e}")))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| CoreError::Message(format!("png encode: {e}")))?;
    }
    Ok(buf)
}

struct ComputerMove;

#[async_trait]
impl Tool for ComputerMove {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "computer_move".to_string(),
            description: "Move the mouse cursor to absolute screen coordinates (pixels)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "x": { "type": "integer" }, "y": { "type": "integer" } },
                "required": ["x", "y"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let x = arg_i32(&args, "x")?;
        let y = arg_i32(&args, "y")?;
        with_enigo(move |e| {
            map_input(e.move_mouse(x, y, Coordinate::Abs))?;
            Ok(json!({ "moved": true, "x": x, "y": y }))
        })
        .await
    }
}

struct ComputerClick;

#[async_trait]
impl Tool for ComputerClick {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "computer_click".to_string(),
            description:
                "Click a mouse button (`left` default, `right`, `middle`). Optionally move \
                 to `x`,`y` first. Intrusive — it takes over the user's pointer."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" }
                }
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let button = match args.get("button").and_then(Value::as_str).unwrap_or("left") {
            "right" => Button::Right,
            "middle" => Button::Middle,
            _ => Button::Left,
        };
        let xy = match (
            args.get("x").and_then(Value::as_i64),
            args.get("y").and_then(Value::as_i64),
        ) {
            (Some(x), Some(y)) => Some((x as i32, y as i32)),
            _ => None,
        };
        with_enigo(move |e| {
            if let Some((x, y)) = xy {
                map_input(e.move_mouse(x, y, Coordinate::Abs))?;
            }
            map_input(e.button(button, Click))?;
            Ok(json!({ "clicked": true }))
        })
        .await
    }
}

struct ComputerType;

#[async_trait]
impl Tool for ComputerType {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "computer_type".to_string(),
            description:
                "Type a string of text at the current focus (intrusive — it sends real keystrokes)."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CoreError::Message("missing required string argument 'text'".to_string())
            })?
            .to_string();
        with_enigo(move |e| {
            map_input(e.text(&text))?;
            Ok(json!({ "typed": text.chars().count() }))
        })
        .await
    }
}

struct ComputerKey;

#[async_trait]
impl Tool for ComputerKey {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "computer_key".to_string(),
            description: "Press a key, optionally with modifiers — e.g. key `c` + modifiers \
                 [`ctrl`] for copy, or key `enter`. `key` is a single character or a named key \
                 (enter, tab, esc, backspace, delete, space, up/down/left/right, home, end, \
                 pageup, pagedown, f1-f12). Modifiers: ctrl, alt, shift, meta."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "modifiers": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["key"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let key_name = args
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CoreError::Message("missing required string argument 'key'".to_string())
            })?
            .to_string();
        let mods: Vec<String> = args
            .get("modifiers")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let key = parse_key(&key_name)?;
        let mod_keys: Vec<Key> = mods
            .iter()
            .map(|m| modifier_key(m))
            .collect::<Result<_>>()?;
        with_enigo(move |e| {
            for m in &mod_keys {
                map_input(e.key(*m, Press))?;
            }
            let r = map_input(e.key(key, Click));
            // Always release modifiers, even if the main key errored.
            for m in mod_keys.iter().rev() {
                let _ = e.key(*m, Release);
            }
            r?;
            Ok(json!({ "pressed": key_name }))
        })
        .await
    }
}

struct ComputerScroll;

#[async_trait]
impl Tool for ComputerScroll {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "computer_scroll".to_string(),
            description: "Scroll the wheel. `amount` lines (negative = up / left); `axis` `vertical` (default) or `horizontal`.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "amount": { "type": "integer" },
                    "axis": { "type": "string", "enum": ["vertical", "horizontal"] }
                },
                "required": ["amount"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let amount = arg_i32(&args, "amount")?;
        let axis = match args
            .get("axis")
            .and_then(Value::as_str)
            .unwrap_or("vertical")
        {
            "horizontal" => Axis::Horizontal,
            _ => Axis::Vertical,
        };
        with_enigo(move |e| {
            map_input(e.scroll(amount, axis))?;
            Ok(json!({ "scrolled": amount }))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_and_modifier_parsing() {
        assert!(matches!(parse_key("enter").expect("k"), Key::Return));
        assert!(matches!(parse_key("f5").expect("k"), Key::F5));
        assert!(matches!(parse_key("a").expect("k"), Key::Unicode('a')));
        assert!(parse_key("notakey").is_err());
        assert!(matches!(modifier_key("ctrl").expect("m"), Key::Control));
        assert!(matches!(modifier_key("cmd").expect("m"), Key::Meta));
        assert!(modifier_key("bogus").is_err());
    }

    #[test]
    fn headless_remediation_is_actionable() {
        let msg = headless_remediation();
        // Names the background-service (session 0) case and tells the user what to do.
        assert!(msg.contains("session 0"));
        assert!(msg.contains("log in"));
    }

    #[test]
    fn png_encodes_rgba() {
        // 1x1 opaque red pixel -> a valid PNG (starts with the PNG signature).
        let png = encode_png(1, 1, &[255, 0, 0, 255]).expect("png");
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    }

    #[tokio::test]
    async fn tools_registered() {
        let mut reg = ToolRegistry::new();
        register_computer(&mut reg);
        // Bad args are rejected before any desktop access is attempted.
        assert!(reg.call("computer_move", json!({ "x": 1 })).await.is_err());
        assert!(reg.call("computer_key", json!({}).clone()).await.is_err());
    }
}
