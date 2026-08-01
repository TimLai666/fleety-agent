## ADDED Requirements

### Requirement: Model responses carry provider-reported token usage

Every model response SHALL be able to carry the token usage the provider reported for that call: input tokens, output tokens, total tokens, and, when the provider reports it, the number of input tokens served from the provider's cache. Usage SHALL be optional: when a provider does not report usage, the response SHALL carry no usage rather than zeros, so that "unknown" is never indistinguishable from "zero tokens". A provider that reports no usage SHALL NOT fail the call.

Each provider SHALL parse its own native usage shape; provider field names SHALL NOT be assumed to be shared across providers.

#### Scenario: a provider that reports usage populates it

- **WHEN** a model call returns a response whose body reports token counts
- **THEN** the model response carries those input, output, and total counts, plus the cached-input count when the provider reported one

#### Scenario: a provider that reports no usage leaves it unknown

- **WHEN** a model call returns a response whose body reports no token counts
- **THEN** the model response carries no usage, the call succeeds, and no zero-valued usage is fabricated

##### Example: per-provider usage field mapping

| Provider family | Input field | Output field | Cached-input field |
| --- | --- | --- | --- |
| OpenAI-compatible chat completions | prompt tokens | completion tokens | cached tokens within the prompt-token details |
| Gemini | prompt token count | candidates token count | cached content token count |
| Codex Responses | input tokens | output tokens | cached tokens within the input-token details |

### Requirement: Streaming calls request usage reporting

When a model call streams, the runtime SHALL ask the provider to report token usage on the final streamed chunk, using whatever request option that provider offers. When a provider does not support such an option, or rejects it, the stream SHALL complete normally with usage left unknown; the runtime SHALL NOT fail or degrade the stream in order to obtain usage.

#### Scenario: streaming call reports usage on completion

- **WHEN** a streaming model call completes against a provider that supports usage reporting on the final chunk
- **THEN** the resulting model response carries that usage

#### Scenario: streaming without usage support still succeeds

- **WHEN** a streaming model call runs against a provider that does not report usage
- **THEN** the stream completes normally and the resulting model response carries no usage

### Requirement: A turn aggregates the usage of its model calls

A completed agent turn SHALL report the aggregate token usage of every model call made during that turn, including calls the loop makes on its own behalf such as context summarization. When no model call in the turn reported usage, the turn SHALL report no usage. When some calls reported usage and others did not, the turn SHALL report the sum of the calls that did.

Aggregated usage SHALL NOT be written into the persisted event stream; it is a property of the turn result.

#### Scenario: a multi-step turn sums its calls

- **WHEN** a turn makes several model calls and each reports usage
- **THEN** the turn result reports the sum of their input, output, total, and cached-input counts

##### Example: summing three calls with one unknown

- **GIVEN** call A reports input=1000 output=50, call B reports no usage, and call C reports input=1200 output=80
- **WHEN** the turn completes
- **THEN** the turn result reports input=2200 and output=130

#### Scenario: a turn with no reported usage reports none

- **WHEN** every model call in a turn returns without usage
- **THEN** the turn result reports no usage rather than zeros
