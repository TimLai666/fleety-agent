## ADDED Requirements

### Requirement: Per-message voice flag

The system SHALL carry a per-message `voice` flag from the client to the core. When the flag is off, the system SHALL NOT request or produce any spoken output and SHALL NOT spend extra tokens on it. The flag SHALL default to off when absent.

#### Scenario: voice off is a normal text turn

- **WHEN** a user message arrives with the voice flag off (or absent)
- **THEN** the turn runs exactly as a non-voice turn, producing no speech and adding no spoken-output instruction to the model

#### Scenario: voice on requests spoken output

- **WHEN** a user message arrives with the voice flag on
- **THEN** the core's system prompt includes the dual-channel instruction so the model can produce a spoken version

### Requirement: Dual-channel output on the terminal turn

When voice is on, the terminal turn SHALL produce two channels from a single model call: a display text and a spoken version. The core SHALL split the model's final message into display and speech at an agreed sentinel: text before the sentinel is the display, text after it is the speech. When the sentinel is absent, the speech SHALL be empty and the display SHALL be the whole message, without error.

#### Scenario: core splits display and speech at the sentinel

- **WHEN** the model's final message contains display text followed by the speech sentinel and a spoken version
- **THEN** the display is the text before the sentinel and the speech is the text after it

##### Example: split outcomes

| Model final message | Display | Speech |
| ------------------- | ------- | ------ |
| `Here is the diff.\n⟦SPEECH⟧\nDone — check the diff.` | `Here is the diff.` | `Done — check the diff.` |
| `All set, no sentinel here.` | `All set, no sentinel here.` | (none) |

### Requirement: Only the terminal turn speaks

Intermediate continuation turns of a goal loop SHALL NOT emit speech to the user. Only the terminal turn — the one that calls `complete_goal` or `ask_user`, or a single-shot turn — SHALL carry the spoken version in its user-facing reply.

#### Scenario: speech rides only the terminal reply

- **WHEN** a voice-on goal loop runs several intermediate continuation turns and then completes
- **THEN** only the final assistant reply carries speech, and no intermediate reply carries speech

### Requirement: Backward-compatible protocol

The new `voice` and `speech` wire fields SHALL be optional so that a client or server that omits them still interoperates, and the protocol version SHALL NOT be bumped for this change.

#### Scenario: old and new peers interoperate

- **WHEN** a message without the `voice` field is received, or an assistant frame without the `speech` field is received
- **THEN** the missing `voice` is treated as off and the missing `speech` is treated as none, and the exchange proceeds normally

##### Example: wire compatibility

| Wire payload | Interpreted as |
| ------------ | -------------- |
| user message with no `voice` field | voice off |
| assistant frame with no `speech` field | no spoken output |
| user message with `voice: true` | voice on |

### Requirement: Terminal OS-native speech with graceful fallback

The terminal SHALL use the operating system's native engines for speech: text-to-speech to read the spoken version aloud, and speech-to-text to turn spoken input into a user message with the voice flag on. When an engine is missing or fails, the terminal SHALL fall back to plain text and SHALL NOT crash. Where the operating system provides no native speech-to-text, the terminal SHALL report that dictation is unavailable and ask the user to type instead.

#### Scenario: missing engine falls back to text

- **WHEN** the OS speech engine is unavailable or returns an error
- **THEN** the terminal continues in plain text — text-to-speech is skipped and speech-to-text prompts the user to type — without crashing

#### Scenario: spoken reply is read aloud

- **WHEN** the terminal receives an assistant reply that carries a spoken version while voice is on
- **THEN** the terminal displays the text and reads the spoken version aloud with the OS engine
