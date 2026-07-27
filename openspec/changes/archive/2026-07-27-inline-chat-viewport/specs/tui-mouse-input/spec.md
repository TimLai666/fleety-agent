## REMOVED Requirements

### Requirement: Mouse reporting is scoped to the Chat workspace

**Reason**: Chat no longer draws the conversation, so the terminal owns its scrolling. Holding mouse reporting would intercept the wheel events the terminal needs to scroll the history Fleety just handed it.

**Migration**: None. Fleety stops sending the enable sequence; the terminal keeps its default mouse handling throughout.

### Requirement: Only Chat receives mouse events

**Reason**: No route receives mouse events now. The input pipeline carries keyboard events only, so the split between a key-only read and an all-events read has nothing left to separate.

**Migration**: `WorkspaceInput::recv` returns keyboard events as before; `recv_event` and the event enum are removed with no replacement.

### Requirement: Mouse events are resolved against the last rendered geometry

**Reason**: Nothing is hit-tested. The conversation is not drawn by Fleety, and the composer no longer takes pointer input.

**Migration**: None. The recorded per-frame geometry is removed.

### Requirement: The wheel scrolls the transcript only over the transcript

**Reason**: The transcript is terminal scrollback, which the terminal scrolls itself.

**Migration**: Users scroll with the terminal's own scrollbar or wheel, which also reaches content older than one screen — something the removed behavior could not do.

### Requirement: Pointer gestures in the composer place the caret and select text

**Reason**: Click-to-position and drag-to-select in the composer were the only remaining use for mouse reporting, and they do not justify overriding the terminal's own selection across the whole screen.

**Migration**: The caret moves with the arrow, word-motion, and line-start/end keys. Selecting text is done with the terminal's own drag, which now works everywhere without a modifier.

### Requirement: Transcript text selection belongs to the terminal

**Reason**: Still true, but its wording assumed mouse reporting was active and told users to hold a modifier. With reporting off, a plain drag selects. The rule is restated without that condition in `inline-terminal-viewport`.

**Migration**: Drag without holding any modifier.
