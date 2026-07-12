//! The `video_extract` tool: scene-aware video understanding.
//!
//! Turns a video — given as a URL or a local file path — into a small set of
//! scene-aware keyframes plus an optional transcript and a manifest, by shelling
//! out to the `claude-real-video` (`crv`) CLI. Unlike the insyra sidecar, `crv`
//! is a one-shot process per call. `crv`/`ffmpeg` are auto-provisioned elsewhere;
//! a missing binary here yields an actionable error, never a panic.
//!
//! `crv` is located via `FLEETY_CRV_BIN`, else on `PATH`. Whisper transcription
//! is opt-in via `FLEETY_VIDEO_WHISPER=on` (matching the provisioning gate), so a
//! `transcribe` request without it degrades to no transcription plus a note.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Default wall-clock ceiling for a single extraction (download + ffmpeg +
/// optional transcription can be slow). `FLEETY_VIDEO_TIMEOUT_SECS` overrides;
/// `0` disables the limit.
const DEFAULT_VIDEO_TIMEOUT_SECS: u64 = 900;

/// Register the `video_extract` tool rooted at `root` (its outputs land under
/// `root`, so the file tools can read them back).
pub fn register_video(registry: &mut ToolRegistry, root: &Path) {
    registry.register(Box::new(VideoExtract {
        root: root.to_path_buf(),
    }));
}

struct VideoExtract {
    root: PathBuf,
}

/// Locate the `crv` binary: env override (when it names a real file), else the
/// bare name for a `PATH` lookup at spawn time (it is a pip console script).
fn locate_crv() -> PathBuf {
    if let Ok(p) = std::env::var("FLEETY_CRV_BIN") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return pb;
        }
    }
    PathBuf::from(if cfg!(windows) { "crv.exe" } else { "crv" })
}

/// Whether Whisper transcription is enabled (`FLEETY_VIDEO_WHISPER=on`, default
/// off). This is the same gate that controls installing the heavy Whisper stack,
/// so the tool only attempts transcription when the operator opted in.
fn whisper_enabled() -> bool {
    std::env::var("FLEETY_VIDEO_WHISPER")
        .map(|v| v.trim().eq_ignore_ascii_case("on"))
        .unwrap_or(false)
}

/// The extraction timeout: `FLEETY_VIDEO_TIMEOUT_SECS` over the default; `0`
/// disables the limit (returns `None`).
fn video_timeout() -> Option<Duration> {
    let secs = std::env::var("FLEETY_VIDEO_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_VIDEO_TIMEOUT_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// The parsed, validated inputs to one extraction (pure so arg-building is
/// unit-testable without a workspace or a live `crv`).
struct CrvParams {
    source: String,
    /// Absolute output directory `crv` writes into.
    out: String,
    scene: Option<f64>,
    fps_floor: Option<f64>,
    max_frames: Option<u64>,
    lang: Option<String>,
    transcribe: bool,
}

/// Build the `crv` argument vector. Transcription is emitted (no `--no-transcribe`)
/// only when the caller asked for it AND Whisper is available; otherwise the
/// `--no-transcribe` flag is added so `crv` never fails for a missing Whisper.
fn build_crv_args(p: &CrvParams, whisper_available: bool) -> Vec<String> {
    let mut args = vec![p.source.clone(), "-o".to_string(), p.out.clone()];
    if let Some(scene) = p.scene {
        args.push("--scene".to_string());
        args.push(format!("{scene}"));
    }
    if let Some(f) = p.fps_floor {
        args.push("--fps-floor".to_string());
        args.push(format!("{f}"));
    }
    if let Some(n) = p.max_frames {
        args.push("--max-frames".to_string());
        args.push(n.to_string());
    }
    if let Some(lang) = &p.lang {
        args.push("--lang".to_string());
        args.push(lang.clone());
    }
    if !(p.transcribe && whisper_available) {
        args.push("--no-transcribe".to_string());
    }
    args
}

/// A filesystem-safe stem derived from the source (last path/URL segment,
/// non-`[A-Za-z0-9._-]` collapsed to `-`, capped), for the default output dir.
fn sanitize_stem(source: &str) -> String {
    let base = source.rsplit(['/', '\\']).next().unwrap_or(source);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(['-', '.']);
    if trimmed.is_empty() {
        "video".to_string()
    } else {
        trimmed.chars().take(60).collect()
    }
}

/// The default workspace-relative output directory for a source.
fn default_out_dir(source: &str) -> String {
    format!("crv-out/{}", sanitize_stem(source))
}

impl VideoExtract {
    /// List the extracted keyframes as workspace-relative paths, sorted.
    fn list_frames(&self, out_abs: &Path, out_rel: &str) -> Vec<String> {
        let frames_dir = out_abs.join("frames");
        let mut frames: Vec<String> = std::fs::read_dir(&frames_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let is_img = name
                    .rsplit('.')
                    .next()
                    .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png"))
                    .unwrap_or(false);
                is_img.then(|| format!("{out_rel}/frames/{name}"))
            })
            .collect();
        frames.sort();
        frames
    }
}

#[async_trait]
impl Tool for VideoExtract {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "video_extract".to_string(),
            description: "Understand a video by extracting SCENE-AWARE keyframes (real visual \
                changes, not fixed-interval samples) plus an optional transcript and a manifest, \
                via the claude-real-video (`crv`) engine. `source` is a video URL (YouTube / \
                Instagram / TikTok / …) OR a workspace file path. Returns the output directory, \
                the manifest text, and the keyframe paths — read the frames with `read_file_bytes` \
                (JPEGs) and the transcript/manifest with `read_file`. Transcription needs Whisper \
                (opt-in: FLEETY_VIDEO_WHISPER=on); without it, frames are still extracted. Wrap in \
                `device_exec` to extract on the device that holds the video."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "video URL or a workspace file path" },
                    "out": { "type": "string", "description": "workspace-relative output dir (default crv-out/<name>)" },
                    "scene": { "type": "number", "description": "scene-change sensitivity 0-1 (crv default 0.30)" },
                    "fps_floor": { "type": "number", "description": "guarantee at least one frame every N seconds (crv default 1.0)" },
                    "max_frames": { "type": "integer", "description": "cap total frames (crv default 150)" },
                    "lang": { "type": "string", "description": "transcription language, e.g. en / zh / auto" },
                    "transcribe": { "type": "boolean", "description": "transcribe audio (needs Whisper; default false)" }
                },
                "required": ["source"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let source = args
            .get("source")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| CoreError::Message("missing required string argument 'source'".into()))?
            .to_string();
        let out_rel = args
            .get("out")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_out_dir(&source));

        // Resolve the output dir under the workspace, block a sensitive location,
        // then ensure the tree (crv also creates its `-o` dir, but this makes the
        // path exist regardless and keeps the outputs readable by the file tools).
        let canon_root = self
            .root
            .canonicalize()
            .map_err(|e| CoreError::Message(format!("workspace unavailable: {e}")))?;
        let out_abs = canon_root.join(&out_rel);
        crate::guard_sensitive(&out_abs)?;
        std::fs::create_dir_all(&out_abs).map_err(|e| {
            CoreError::Message(format!("cannot create output dir '{out_rel}': {e}"))
        })?;

        let transcribe = args
            .get("transcribe")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let whisper = whisper_enabled();
        let params = CrvParams {
            source: source.clone(),
            out: out_abs.to_string_lossy().to_string(),
            scene: args.get("scene").and_then(Value::as_f64),
            fps_floor: args.get("fps_floor").and_then(Value::as_f64),
            max_frames: args.get("max_frames").and_then(Value::as_u64),
            lang: args.get("lang").and_then(Value::as_str).map(str::to_string),
            transcribe,
        };
        let crv_args = build_crv_args(&params, whisper);
        let bin = locate_crv();

        let mut cmd = tokio::process::Command::new(&bin);
        cmd.args(&crv_args)
            .current_dir(&self.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let spawned = cmd.spawn().map_err(|e| {
            CoreError::Message(format!(
                "cannot start the video engine ('{}'): {e}. Install it with `pipx install \
                 claude-real-video` (or set FLEETY_CRV_BIN), and ensure ffmpeg is on PATH.",
                bin.display()
            ))
        })?;

        let output = match video_timeout() {
            Some(dur) => match tokio::time::timeout(dur, spawned.wait_with_output()).await {
                Ok(r) => r.map_err(|e| CoreError::Message(format!("video engine failed: {e}")))?,
                Err(_) => {
                    return Err(CoreError::Message(format!(
                        "video extraction timed out after {}s and was terminated (raise \
                         FLEETY_VIDEO_TIMEOUT_SECS, or 0 to disable)",
                        dur.as_secs()
                    )));
                }
            },
            None => spawned
                .wait_with_output()
                .await
                .map_err(|e| CoreError::Message(format!("video engine failed: {e}")))?,
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr.lines().rev().take(8).collect::<Vec<_>>().join("\n");
            return Err(CoreError::Message(format!(
                "video extraction failed (exit {}). If this is a missing dependency, ensure \
                 ffmpeg is installed and reachable.\n{}",
                output
                    .status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                tail
            )));
        }

        let manifest = std::fs::read_to_string(out_abs.join("MANIFEST.txt")).unwrap_or_default();
        let frames = self.list_frames(&out_abs, &out_rel);
        let transcript_abs = out_abs.join("transcript.txt");
        let transcript_path = transcript_abs
            .is_file()
            .then(|| format!("{out_rel}/transcript.txt"));

        let mut notes: Vec<String> = Vec::new();
        if transcribe && !whisper {
            notes.push(
                "transcription skipped (Whisper not enabled; set FLEETY_VIDEO_WHISPER=on)"
                    .to_string(),
            );
        }

        Ok(json!({
            "out_dir": out_rel,
            "manifest_path": format!("{out_rel}/MANIFEST.txt"),
            "manifest": manifest,
            "frames": frames,
            "frame_count": frames.len(),
            "transcript_path": transcript_path,
            "notes": notes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(source: &str, transcribe: bool) -> CrvParams {
        CrvParams {
            source: source.to_string(),
            out: "/ws/crv-out/x".to_string(),
            scene: None,
            fps_floor: None,
            max_frames: None,
            lang: None,
            transcribe,
        }
    }

    #[test]
    fn build_args_defaults_add_no_transcribe() {
        // No transcription requested → --no-transcribe, minimal args.
        let a = build_crv_args(&params("vid.mp4", false), false);
        assert_eq!(
            a,
            vec![
                "vid.mp4".to_string(),
                "-o".into(),
                "/ws/crv-out/x".into(),
                "--no-transcribe".into()
            ]
        );
    }

    #[test]
    fn build_args_transcribe_only_when_whisper_available() {
        // Requested + Whisper present → no --no-transcribe.
        let a = build_crv_args(&params("v", true), true);
        assert!(!a.iter().any(|x| x == "--no-transcribe"));
        // Requested but Whisper absent → still --no-transcribe (crv won't fail).
        let a = build_crv_args(&params("v", true), false);
        assert!(a.iter().any(|x| x == "--no-transcribe"));
    }

    #[test]
    fn build_args_passes_through_knobs() {
        let p = CrvParams {
            source: "u".into(),
            out: "o".into(),
            scene: Some(0.5),
            fps_floor: Some(2.0),
            max_frames: Some(80),
            lang: Some("zh".into()),
            transcribe: true,
        };
        let a = build_crv_args(&p, true);
        // Adjacent flag/value pairs are present in order.
        let joined = a.join(" ");
        assert!(joined.contains("--scene 0.5"));
        assert!(joined.contains("--fps-floor 2"));
        assert!(joined.contains("--max-frames 80"));
        assert!(joined.contains("--lang zh"));
        assert!(!joined.contains("--no-transcribe"));
    }

    #[test]
    fn default_out_dir_sanitizes_source() {
        assert_eq!(default_out_dir("clip.mp4"), "crv-out/clip.mp4");
        assert_eq!(
            default_out_dir("https://youtube.com/watch?v=abc"),
            "crv-out/watch-v-abc"
        );
        // A path takes the last segment.
        assert_eq!(default_out_dir("/a/b/movie.mkv"), "crv-out/movie.mkv");
        // Degenerate input still yields a usable name.
        assert_eq!(default_out_dir("///"), "crv-out/video");
    }

    #[test]
    #[serial_test::serial]
    fn locate_crv_honors_env_override_when_file_exists() {
        // A non-file override is ignored (falls back to the bare name).
        std::env::set_var("FLEETY_CRV_BIN", "/definitely/not/here/crv");
        let p = locate_crv();
        assert!(matches!(p.to_str(), Some("crv") | Some("crv.exe")));
        std::env::remove_var("FLEETY_CRV_BIN");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn missing_crv_returns_actionable_error_not_panic() {
        let dir = std::env::temp_dir().join(format!("fleety-vid-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk root");
        // Point crv at a nonexistent absolute file so it isn't a real file (env
        // ignored) and spawning the bare name fails without a real crv on PATH.
        std::env::set_var(
            "FLEETY_CRV_BIN",
            dir.join("no-such-crv").display().to_string(),
        );
        std::env::set_var("FLEETY_VIDEO_TIMEOUT_SECS", "5");
        let tool = VideoExtract { root: dir.clone() };
        let err = tool
            .call(json!({ "source": "clip.mp4" }))
            .await
            .expect_err("no crv on PATH");
        let msg = err.report().message;
        assert!(
            msg.contains("video engine") || msg.contains("claude-real-video"),
            "unexpected error message: {msg}"
        );
        std::env::remove_var("FLEETY_CRV_BIN");
        std::env::remove_var("FLEETY_VIDEO_TIMEOUT_SECS");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
