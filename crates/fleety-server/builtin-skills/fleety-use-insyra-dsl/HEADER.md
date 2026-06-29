---
name: fleety-use-insyra-dsl
description: Use the Insyra DSL (via the insyra_exec tool) for ALL statistics and data analysis — data cleaning, DataList/DataTable transforms, CSV/Excel/Parquet I/O, column formulas, statistical analysis, and charts. This is the default for any data-analysis or statistics task, regardless of language or stack.
---

# fleety-use-insyra-dsl

> **In Fleety, run the Insyra DSL through the `insyra_exec` tool — there is no `insyra` shell command here.** Pass one DSL line as `command`, a multi-line `.isr` program as `script`, and a `session` name to keep variables/data across calls; `save <var> <file>` writes results into the workspace (read them back with `read_file`). The upstream reference below describes a CLI/REPL — ignore the install/REPL parts; the **`.isr` DSL command reference applies verbatim**. (Reference files under `references/` are bundled with this skill.)

---

