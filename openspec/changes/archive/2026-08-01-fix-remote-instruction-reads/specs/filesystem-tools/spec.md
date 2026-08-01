## ADDED Requirements

### Requirement: A line-numbered view can be reversed to the original text

Because the slice-returning read tools emit only a line-numbered view, the system SHALL provide a single shared function that reverses that view back to the original text, and every caller that needs the raw bytes SHALL use it rather than reimplementing the parsing.

The reversal SHALL strip the line-number prefix up to and including the first tab on each line, and SHALL leave a line that contains no tab unchanged, so that content which itself contains tabs is not corrupted. Reversing the numbered view of a text SHALL yield that text.

A single shared function is required, not per-call-site parsing, so that the numbering format and its inverse cannot drift apart.

#### Scenario: round-tripping a slice returns the original

- **WHEN** a text is rendered as a line-numbered view and then reversed
- **THEN** the result equals the original text

##### Example: round trip with a tab inside the content

- **GIVEN** the text `alpha` newline `beta\tgamma` (the second line contains a literal tab)
- **WHEN** it is numbered from line 1 and then reversed
- **THEN** the result is exactly `alpha` newline `beta\tgamma`

#### Scenario: a line with no tab is left alone

- **WHEN** the reversal encounters a line containing no tab
- **THEN** that line is returned unchanged rather than being truncated or dropped
