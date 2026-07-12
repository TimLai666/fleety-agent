## ADDED Requirements

### Requirement: Native scene-aware video extraction tool

The agent SHALL have a `video_extract` tool that turns a video — given as a URL or a local file path — into a small set of scene-aware keyframes plus an optional transcript and a manifest, by invoking the `claude-real-video` (`crv`) command. The tool SHALL accept `source` (required) and optional `out`, `scene`, `fps_floor`, `max_frames`, `lang`, and `transcribe`. It SHALL resolve `out` under the workspace so the outputs are readable by the file tools, and SHALL return a structured result naming the output directory, the manifest path and its text, the list of keyframe paths, the frame count, and the transcript path when one was produced. Its risk class SHALL be `Mutate` (it writes files and may download over the network). The argument assembly from the parameters SHALL be a pure, unit-testable function.

#### Scenario: extract keyframes from a source

- **WHEN** the agent calls `video_extract` with a source URL or local path
- **THEN** the tool runs `crv` and returns the output directory, manifest text, and the keyframe paths, which the agent reads with the file tools (binary-safe for the JPG frames)

#### Scenario: transcription is opt-in and degrades gracefully

- **WHEN** `transcribe` is true but the Whisper dependency is not installed
- **THEN** the tool runs without transcription and its result notes that transcription was skipped, rather than failing

### Requirement: The extraction tool is available on every device

The `video_extract` tool SHALL be registered both on the server and inside every device's local registry, so it is advertised by each daemon and routable with `device_exec`. Extraction SHALL therefore run on whichever device holds the video, not only on the server.

#### Scenario: extract on a specific device

- **WHEN** the agent routes `video_extract` to a device via `device_exec` and that device has the extraction dependencies
- **THEN** the extraction runs on that device and returns its result to the server

### Requirement: Extraction dependencies are auto-provisioned on the server

The server SHALL best-effort provision the extraction dependencies without operator action: the `claude-real-video` package (and its `yt-dlp` dependency) via the Python package path, and `ffmpeg` via the platform package manager (winget / brew / apt). Provisioning SHALL be opt-out per dependency via its `FLEETY_*_AUTO_INSTALL` variable and SHALL never block or crash server startup. Whisper-based transcription SHALL be opt-in via `FLEETY_VIDEO_WHISPER` (default off), so the heavy transcription stack is not installed unless requested. `FLEETY_CRV_BIN` and `FLEETY_FFMPEG_BIN` SHALL override the resolved binary paths.

#### Scenario: stock server provisions extraction without configuration

- **WHEN** a server starts with auto-install enabled and the extraction binaries absent
- **THEN** it attempts to install `crv` (Python package) and `ffmpeg` (OS package manager) in the background, best-effort, without blocking startup

#### Scenario: a missing binary yields an actionable error, not a crash

- **WHEN** `video_extract` runs but `crv` or `ffmpeg` cannot be resolved
- **THEN** the tool returns a readable, actionable error (how to install, or to set the override variable) and does not panic

### Requirement: A builtin skill guides video extraction

A builtin skill SHALL ship in-binary that directs the agent to use the `video_extract` tool for videos (not a bare CLI), covering when to transcribe, how to read the returned frames and transcript, and how to route extraction to another device. The vendored skill SHALL retain the upstream project's MIT license attribution.

#### Scenario: the skill is seeded and points at the tool

- **WHEN** the server seeds its builtin skills at boot
- **THEN** the video skill is present and instructs the agent to call `video_extract` rather than invoking `crv` directly
