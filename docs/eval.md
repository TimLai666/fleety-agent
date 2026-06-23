# Fleety eval — golden conversation harness

`fleety-eval` is an offline regression harness for the agent loop. It replays
recorded *golden* conversations — fresh workspace, fixed user input, fixed
scripted assistant responses — against the real tool registry, and asserts
that the loop's behaviour (which tools ran, what the final reply says, what
ended up in the workspace) still matches.

It runs every CI build and gates merge: if a refactor accidentally drops a
workspace tool, changes how `write_file` produces side effects, or stops a
multi-step task from completing, the relevant golden fails.

## Running it locally

```bash
cargo run -p fleety-eval -- run crates/fleety-eval/goldens
# or just one file
cargo run -p fleety-eval -- run crates/fleety-eval/goldens/workspace.jsonl
```

Exit code is the number of failed goldens (capped at 255). `0` means all
green.

Output:

```
PASS  read_file_answers  (2 steps, tools: read_file)
FAIL  edit_replaces_text
        forbidden tool 'write_file' was called (actual: ["edit_file", "write_file"])
```

## Golden format

Goldens live in `.jsonl` files — **one JSON object per line**, no pretty
printing. Each object is a `Golden` (see
[`crates/fleety-eval/src/golden.rs`](../crates/fleety-eval/src/golden.rs)).

Fields:

| field | type | meaning |
|---|---|---|
| `name` | string | unique, used in output |
| `description` | string (optional) | what the golden tests |
| `workspace_files` | `{path: content}` | files seeded into a fresh temp workspace |
| `user_input` | string | the user message that drives the loop |
| `system_prompt` | string (optional) | optional system message prepended |
| `scripted_responses` | array | one per loop step (assistant text + tool calls) |
| `expected.tools_called` | string[] | these tool names must run, **in order** (other calls may appear between) |
| `expected.must_not_call` | string[] | none of these tools may run |
| `expected.final_contains` | string[] | the last assistant message must contain every substring |
| `expected.workspace_files_after` | `{path: content}` | files in the workspace after the run must exactly equal this |
| `expected.workspace_files_absent` | string[] | these paths must not exist after the run |

### How scripted responses work

The runner spins up a [`MockProvider`](../crates/agent-core/src/model.rs) seeded
with `scripted_responses` and lets the real
[`run_turn`](../crates/agent-core/src/agent.rs) loop drive everything else:

- Each `ScriptedResponse` becomes one `complete()` call's return value
- Empty `tool_calls` means "terminal answer" — loop ends after that step
- Tool calls run against the **real** tool registry against the seeded temp
  workspace (so side effects are real and assertable)
- The next scripted response feeds the next loop step regardless of what
  came back from the tool — so don't script around tool results you don't
  know in advance (the loop and your script aren't conversing, the script
  drives both ends)

This means goldens are **deterministic but not adaptive**. They're great for
locking in known good sequences (regression testing) and bad for testing
how a real model would react to unexpected tool errors (use an integration
test for that).

## Adding a golden

1. Pick the scenario you want to lock in. Keep it focused — one behaviour
   per golden.
2. Append a JSON object as a new line in the relevant `.jsonl` file (or
   create a new file under `crates/fleety-eval/goldens/`).
3. Run `cargo run -p fleety-eval -- run crates/fleety-eval/goldens/<file>.jsonl`
   and iterate until it passes.
4. Commit. The next CI run will gate it.

## Limitations (intentional)

- **No real model calls.** `fleety-eval` is offline-first; if you want to
  measure real-model behaviour (cost, latency, accuracy), build a separate
  online eval harness — this one is the regression gate.
- **No cross-process scenarios.** Recovery flows (scheduler tick, journal
  reconstruction) and multi-device tool routing have their own integration
  tests in `fleety-server`. The MockProvider model can't represent the
  multi-process state machine those need.
- **Only workspace tools today.** The current registry is
  [`fleety_tools::register_workspace`](../crates/fleety-tools/src/lib.rs).
  When other tool surfaces (skills, MCP, web, etc.) need coverage, either
  register them inside the runner or write a sibling crate that does.
