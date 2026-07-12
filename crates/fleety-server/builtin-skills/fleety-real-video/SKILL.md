---
name: fleety-real-video
description: Understand a video (a URL or a local file) by extracting scene-aware keyframes plus an optional transcript, instead of sampling frames blindly or shipping the whole file to the model. Use whenever the user shares a video link (YouTube / Instagram / TikTok / …) or a video file and wants it summarised, searched, transcribed, or reasoned about.
---

# Real video understanding

Most "watch this video" attempts either read only the transcript or grab a frame every N seconds — missing fast cuts and over-sampling static shots. Fleety instead detects the frames that actually *matter* (scene cuts, text/slide changes, gestures) and pairs them with a transcript, so you can reason about a video without burning context on the raw file.

Do this with the **`video_extract` tool** — do NOT invoke the `crv` CLI yourself. The tool wraps it, resolves outputs into the workspace, and returns structured paths.

## How to use it

1. Call `video_extract` with `source` = the video URL **or** a workspace file path. That is usually all you need — the defaults (scene sensitivity, frame cap, one-frame-per-second floor) are tuned for LLM consumption.
2. It returns `{ out_dir, manifest_path, manifest, frames[], frame_count, transcript_path?, notes[] }`.
3. Read the results:
   - `manifest` is already in the result — it summarises what was found; skim it first.
   - Read the keyframes with **`read_file_bytes`** (they are JPEGs — the binary-safe tool), not `read_file`.
   - If `transcript_path` is present, read it with `read_file`.
4. Answer from the frames + transcript. Cite timestamps from the manifest/transcript when the user asks "where".

### Useful parameters

- `transcribe: true` — get spoken audio as text. Needs Whisper, which is **opt-in**: the operator must set `FLEETY_VIDEO_WHISPER=on` (it installs a heavy stack). If it is off, the tool still extracts frames and notes that transcription was skipped — mention that to the user if they wanted the words.
- `lang` — transcription language hint (`en` / `zh` / `auto`).
- `max_frames`, `scene`, `fps_floor` — only tune these if the default frame set is too sparse or too dense for the question.
- `out` — a workspace-relative output dir, if you want a stable location; otherwise a sensible default under `crv-out/` is chosen.

## On another device

The tool runs on every device. If the video lives on a specific device (a phone, a NAS, a laptop), extract it *there* by wrapping the call: `device_exec(device: "<id>", tool: "video_extract", args: { source: "…" })`. That device needs the engine (`crv`) and `ffmpeg`; the server auto-provisions them, and on a bare device you may need to install them (the tool returns an actionable error if they're missing).

## When NOT to use it

- A pure audio file with no visuals → a transcript tool / `--no-transcribe` off is enough; frames add nothing.
- A single image → just read it directly.
- The user only wants the raw file passed to the model as an attachment → use the multimodal `--video` path instead.

---

*Powered by [claude-real-video](https://github.com/HUANGCHIHHUNGLeo/claude-real-video) (MIT License, © its authors), integrated into Fleety as the `video_extract` tool.*
