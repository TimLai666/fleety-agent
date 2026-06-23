# use-codebase-memory

> **In Fleety, reach for `codebase-memory` *first* whenever the task involves
> understanding a codebase — finding code, tracing call paths, judging the blast
> radius of a change, hunting dead code, or answering "where is X defined / who
> uses Y." It is dramatically cheaper and more precise than grep/find/Read across
> a real-sized repo.**

Fleety ships a built-in MCP server named `codebase-memory` that exposes a
sub-millisecond code knowledge graph (BM25 + vector search + Cypher + call-graph
traversal + impact analysis). You call its tools through `mcp_call`:

```
mcp_call(server="codebase-memory", tool="<name>", arguments={...})
```

## When to use which tool

| Task | Tool |
|---|---|
| Index a workspace before the first query | `index_repository` |
| Free-text / regex / semantic search across code | `search_graph` |
| Grep-style with structural context (signatures + neighbours) | `search_code` |
| Read a function/method by qualified name | `get_code_snippet` |
| Find who calls / who's called by X (call graph) | `trace_path` |
| Run a precise multi-hop Cypher query | `query_graph` |
| Map a git diff to affected symbols + risk | `detect_changes` |
| High-level architecture, layers, hotspots, clusters | `get_architecture` |
| Discover schema (labels/edges/properties) | `get_graph_schema` |
| Persist/recall a decision (ADR) across sessions | `manage_adr` |
| List/delete indexed projects, check index status | `list_projects` / `delete_project` / `index_status` |

## First-use bootstrap

Before any query against a workspace, the project must be indexed once:

```
mcp_call(server="codebase-memory", tool="index_repository",
         arguments={"repo_path": "<absolute path to repo root>",
                    "mode": "full"})
```

Modes: `full` (best quality, includes similarity + semantic edges),
`moderate` (filtered, same edges), `fast` (filtered, no similarity/semantic),
`cross-repo-intelligence` (skip extraction, just match cross-service edges).

After the first index, codebase-memory's background watcher keeps the graph
fresh as files change — you don't need to re-index by hand.

## Default play

When you're about to grep a real repo, **stop and ask whether one of these would
answer the same question for ~1% of the tokens**:

* "Where is `<symbol>` defined / what calls it?" → `search_graph` (regex /
  semantic) then `trace_path` direction=`inbound`.
* "What breaks if I change this function?" → `detect_changes` for a git diff,
  or `trace_path` direction=`inbound` depth=3+.
* "What's the architecture of this service?" → `get_architecture`.
* "Show me dead code." → `query_graph` with
  `MATCH (f:Function) WHERE NOT EXISTS { (f)<-[:CALLS]-() } RETURN f.name`.
* "Hot paths / loops worth optimising." → `query_graph` with
  `WHERE f.transitive_loop_depth >= 3`.

If the project hasn't been indexed yet, run `index_repository` first — the
overhead is one-time and pays back on the very first query.

## Falling back

`codebase-memory-mcp` is a separate binary provisioned by `fleetyd
install/update`. If it isn't installed, `mcp_call` returns a clear spawn error;
that's the cue to tell the user to run `fleetyd update`, *not* to silently fall
back to manual grep for a task that needs graph context.

For trivial single-file searches or non-code text scans, the workspace tools
(`search_files`, `read_file`) remain the right answer — codebase-memory is the
*structural* answer, not a strict replacement.
