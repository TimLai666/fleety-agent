## ADDED Requirements

### Requirement: Long strings are truncated head-and-tail

When the structural compressor truncates a string longer than its string threshold, it SHALL retain both a head segment (a leading portion) and a tail segment (a trailing portion) of the original, joined by an omission marker stating how many characters were dropped — mirroring the head-and-tail trimming already applied to long arrays. This keeps the end of a long single-string output visible (a command's stdout/stderr, for instance, where errors and summaries commonly sit at the end). A string at or below the threshold SHALL be returned unchanged. Truncating a string this way SHALL still count as content loss, so the retrievable-result id marker is attached exactly as it is for any other truncation.

#### Scenario: a long string keeps its head and tail

- **WHEN** a string far longer than the string threshold is structurally compressed
- **THEN** the output contains both the original's opening text and its closing text, with an omission marker naming the dropped character count between them

#### Scenario: a short string is unchanged

- **WHEN** a string at or below the string threshold is structurally compressed
- **THEN** it is returned unchanged

##### Example: head and tail retained

- **GIVEN** a string of 10000 identical head characters followed by a distinct 4-character tail, with a string threshold of 4000
- **WHEN** it is structurally compressed
- **THEN** the output begins with the head characters, ends with the distinct tail characters, and carries an omission marker for the dropped middle

## MODIFIED Requirements

### Requirement: Full tool results are retrievable in bounded segments

The system SHALL provide a `fetch_tool_result(id, offset?, limit?)` tool that returns the full retained result for an id as a character window (`offset`..`offset`+`limit`), reporting the total length and the next offset (null at the end). A single fetch SHALL NOT exceed the standard tool-result character budget. To ensure a fetched page reaches the model intact, `limit` SHALL default to and be capped at the structural string-compression threshold (the size at or below which the structural compressor does not truncate a string), so that when a fetched page is fed back through the normal tool-result compression it is NOT re-truncated and does NOT grow a self-referential fetch marker. A large result can therefore only be read by paging, and retrieval can never re-exceed the context budget the truncation protected.

#### Scenario: paging through a large result

- **WHEN** the agent calls `fetch_tool_result` for a large result and follows `next_offset` until it is null
- **THEN** it receives the whole result across bounded segments, each reaching the model intact, with an accurate total length

#### Scenario: a fetched page is not re-truncated when fed back

- **WHEN** a fetched page is returned and passes back through the normal tool-result compression
- **THEN** its content reaches the model unchanged, with no further truncation and no `fetch_tool_result` marker pointing at the fetch itself

#### Scenario: a fetch limit is clamped to the threshold

- **WHEN** a caller requests a `limit` larger than the string-compression threshold
- **THEN** the returned window is clamped to that threshold

#### Scenario: offset past the end

- **WHEN** the offset is at or beyond the total length
- **THEN** the content is empty, the next offset is null, and the reported total is correct
