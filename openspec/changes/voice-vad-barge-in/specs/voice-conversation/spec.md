## ADDED Requirements

### Requirement: Voice-activity endpointed capture

The terminal SHALL capture a spoken utterance using voice-activity detection: recording SHALL begin listening immediately, SHALL treat sustained microphone energy above a configured threshold as speech, and SHALL end the utterance once trailing silence exceeds a configured hangover duration. The capture SHALL enforce a maximum-utterance duration cap and a start-of-speech timeout; on reaching either cap it SHALL return whatever was captured, or none when no speech was ever detected, without crashing. When voice-activity detection is disabled by configuration, the terminal SHALL fall back to fixed-duration recording.

#### Scenario: silence ends the utterance

- **WHEN** a user speaks and then stops for longer than the configured trailing-silence hangover
- **THEN** recording ends and the captured audio up to that point is returned for transcription or upload

#### Scenario: maximum-duration cap stops a long utterance

- **WHEN** a user keeps speaking past the configured maximum utterance duration
- **THEN** recording stops at the cap and returns the captured audio without error

#### Scenario: no speech within the start timeout returns nothing

- **WHEN** no microphone energy above the threshold is detected within the configured start-of-speech timeout
- **THEN** capture ends and returns none, so the caller prompts the user to type instead, without crashing

#### Scenario: disabling voice-activity detection restores fixed-duration recording

- **WHEN** voice-activity detection is turned off by configuration
- **THEN** the terminal records for the fixed configured number of seconds exactly as before

### Requirement: Barge-in during spoken playback

While the terminal is reading a spoken reply aloud, it SHALL monitor the microphone for the onset of user speech and, on detecting it, SHALL stop playback immediately and proceed to capture the user's utterance. The onset test SHALL require sustained energy across several consecutive analysis windows so that transient noise and speaker echo do not falsely interrupt playback. Barge-in SHALL be configurable on or off; when off, or when no microphone or speech engine is available, the terminal SHALL play the reply to completion without crashing.

#### Scenario: user speech interrupts playback

- **WHEN** the user begins speaking while a spoken reply is being read aloud and barge-in is enabled
- **THEN** playback stops immediately and the terminal proceeds to capture the user's next utterance

#### Scenario: uninterrupted playback runs to completion

- **WHEN** no sustained user speech is detected while a spoken reply is read aloud
- **THEN** the reply is read to completion and the terminal then listens for the next utterance

#### Scenario: barge-in disabled or unavailable never interrupts

- **WHEN** barge-in is turned off by configuration, or no microphone or speech engine is available
- **THEN** the reply is played to completion before the terminal listens again, without crashing
