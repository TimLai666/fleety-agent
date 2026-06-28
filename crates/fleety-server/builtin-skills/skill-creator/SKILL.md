---
name: skill-creator
description: Create, edit, and improve Fleety skills. Use whenever the user asks to build, edit, or optimise a skill, when capturing a repeatable workflow you just did as a reusable skill, or when the learning loop nudges you to save what you learned after a complex task.
---

# Skill Creator

A skill for making your own skills and iteratively improving them. Reach for it when the user asks to build/edit/optimise a skill, when you want to capture a workflow you just did so you never have to re-derive it, or when the learning loop nudges you after a complex task to save what you learned.

The loop is: decide what the skill should do → draft it → try it on a couple of realistic prompts → review with the user → improve → repeat until it's good. Figure out where the user is in that loop and jump in. Stay flexible — if they just want to "vibe it out" without formal testing, do that.

## How skills work in Fleety (read this first — it differs from other hosts)

A skill is a **directory** with a `SKILL.md` plus optional bundled files. You create and edit skills entirely through the `skill_*` tools — never by writing raw files or "packaging" anything.

- **Three tiers**, merged by name with precedence **installed > authored > builtin**:
  - **builtin** — shipped in the binary, read-only.
  - **authored** — skills *you* write for yourself. A `skill_write_file` to a name that doesn't exist yet lands here automatically. You own these; edit them freely.
  - **installed** — user-chosen packs. Only create/edit/remove these when the user asks.
- **SKILL.md opens with YAML frontmatter** holding `name` and `description` (the Agent Skills standard). Fleety shows the `description` in `list_skills` and triggers off it, so it must say what the skill does AND when to use it. (A skill with no frontmatter still loads — Fleety falls back to the first body line — but always write proper frontmatter.)
- **The directory name is the skill name** (what you pass to the `skill_*` tools); the frontmatter `name` should match it.

**Which tier — who is the skill for? Decide this before you start.** It changes which tool you use and where the skill lands:

- **The user asks you to make or install a skill for them** → it's a USER skill, so put it in the **installed** tier: create it with `skill_install` (pass the SKILL.md body as `content`; `from_url`/`from_path` also work), then add any extra files (scripts/references) with `skill_write_file` for that same name (now that it exists in `installed`, writes go there). Edit or remove an installed skill only when the user asks.
- **You're capturing something for your own future use** (the learning loop nudged you, or you noticed a repeatable workflow worth keeping) → it's an **authored** skill: just `skill_write_file` a new name and it lands in `authored` automatically.
- **builtin** skills are read-only. Two ways to build on one: to *replace or override* a built-in's own behaviour, author or install a skill of the **same name** (it shadows the built-in); to *add to, specialise, or combine* what it does, just make a **new skill with a different name** that complements it — you don't have to shadow it, and the built-in stays available.

Either way the SKILL.md format and the writing guidance below are identical — only the tier and the initial tool differ.

Tools you use:
- `skill_write_file` (create/overwrite a file — SKILL.md, a script, a reference), `skill_edit_file` (precise substring / line-range edit), `skill_read_file`, `skill_list_files`, `skill_delete_file`, `skill_remove` (whole pack).
- `list_skills` reports each skill's `source` and on-disk `path` — you need that `path` to run a bundled script (see below).

## Creating a skill

### Capture intent

Understand what the user actually wants before drafting. The current conversation often already contains the workflow to capture (e.g. "turn what you just did into a skill") — mine it first: which tools were used, the sequence of steps, corrections the user made, the input/output shapes. Then confirm the gaps:

1. What should this skill let the agent do?
2. When should it trigger — what phrasings/contexts? (this drives the description)
3. What's the expected output?
4. Is it worth setting up a couple of test prompts? Skills with objective outputs (file transforms, data extraction, fixed workflows) benefit; subjective ones (writing style) usually don't. Suggest a sensible default, let the user decide.

### Write the SKILL.md

Write it with `skill_write_file` (name = the skill, file = `SKILL.md`). Structure:

- **The frontmatter `description` is the trigger.** In the `---` block write a `description` covering what it does AND when to use it. Agents tend to *under*-trigger skills, so make it a little pushy: name the concrete situations, not just the capability. E.g. not "Build a dashboard." but "Build a fast dashboard for fleet metrics. Use whenever the user mentions dashboards, metrics, or wants to visualise device data, even if they don't say 'dashboard'." Keep `description` ≤1024 chars; `name` is lowercase letters/digits/hyphens, ≤64 chars, no `anthropic`/`claude`, matching the directory name.
- **Body = imperative instructions.** Tell the agent what to do, and **explain the why** behind each instruction — today's models have good theory of mind and follow reasoning far better than rote `ALWAYS`/`NEVER`. If you catch yourself writing rigid all-caps rules, that's a yellow flag: reframe and explain instead.
- Keep it general, not overfit to one example. Draft it, then reread with fresh eyes and tighten.

#### SKILL.md format — copy this shape

SKILL.md opens with YAML frontmatter (`name` + `description`), then the instructions. The skeleton:

````
---
name: <skill-name>
description: <what it does> — use when <concrete trigger situations>
---

# <Skill Name>

<one or two sentences: what this skill is for and the rough approach>

## <First step or section, written as a verb>
- <do X — and, briefly, why it matters>
- <do Y>

## <Next section>
- <…>
````

A complete, valid minimal example you can pattern-match against:

````
---
name: device-health-check
description: Snapshot a device's disk, memory, and failed services. Use when the user asks whether a box is healthy, why it's slow, or before deploying to it.
---

# Device Health Check

Gather a quick health picture of one device and report problems first, healthy signals second.

## Collect
- Run `df -h`, `free -m`, and `systemctl --failed` on the target device (wrap each in device_exec for a remote box). These three cover the usual culprits — full disk, memory pressure, crashed services.
- Only when something looks off, read `references/triage.md` for the deeper checklist.

## Report
- Lead with anything wrong (disk >90%, failed units), then the all-clear items. Name the device on every line so nothing gets confused across machines.
````

That example is self-contained; a real skill adds `references/triage.md` (read on demand) or a `scripts/` helper only if they earn their place. Keep the SKILL.md body lean — push depth into `references/`.

### Anatomy and progressive disclosure

```
skill-name/
├── SKILL.md            (required; YAML frontmatter, then instructions)
└── (optional)
    ├── scripts/        executable helpers for deterministic/repeated work
    ├── references/     docs the agent reads only when SKILL.md points to them
    └── assets/         files used in output (templates, etc.)
```

Loading is layered, so spend the context budget wisely:
1. The frontmatter description is always in context — keep it tight.
2. The SKILL.md body loads when the skill is used — aim under ~500 lines.
3. Bundled files load only when referenced — push detail into `references/` and point to it from SKILL.md ("for the X format, read `references/x.md`"). For a long reference (>~300 lines) give it a table of contents. When a skill spans several variants, split by variant so the agent reads only the relevant file.

### Bundled scripts — let a skill carry its own tools

When a skill needs a deterministic or repetitive step, don't make the agent re-derive code every time — bundle a script under `scripts/` (write it with `skill_write_file`) and tell the skill how to run it. Fleety has no dedicated "run skill script" tool; run it through `run_command`:

1. Get the skill's directory from `list_skills` (its `path` field).
2. Run `run_command` with the interpreter and the absolute path, e.g. `python "<path>/scripts/foo.py" …`.
3. To run it on another device, wrap that in `device_exec`.

Only bundle a script when it genuinely earns its place — a step that's exact, repeated, and better as code than as prose. State its inputs/outputs in SKILL.md so the agent calls it correctly.

### Examples and output formats

Examples pull their weight. A compact form:

```
## Commit message format
Example — Input: "added JWT auth"  →  Output: "feat(auth): implement JWT-based authentication"
```

To pin an output shape, show the exact template the agent should fill in rather than describing it abstractly.

### Principle of lack of surprise

A skill must do what its description says and nothing sneaky. Never put malware, exploit code, credential exfiltration, or detection-evasion into a skill, and don't build skills meant to mislead the user or enable unauthorised access. (Benign roleplay-style skills are fine.)

## Format limits and validation

Path rules Fleety enforces (a write that breaks these is rejected):
- The skill's **directory name** is non-empty and contains no `/`, `\`, `..`, or leading `.`.
- In-skill **file paths** stay inside the skill — no absolute paths, no `..`.
- `SKILL.md` must exist and be a real file (not a symlink).

Agent Skills format rules (checked by `skill_validate`):
- SKILL.md opens with YAML frontmatter containing `name` and `description`.
- `name`: ≤64 chars, lowercase letters/digits/hyphens only, no `anthropic`/`claude`, and should match the directory name.
- `description`: non-empty, ≤1024 chars, and says what + when.
- Keep the body lean; move detail into reference files.

Before you call a skill done, run `skill_validate` — pass `content` to check a draft before writing, or `name` to check what you wrote. It returns `ok` plus `issues`: errors (no frontmatter, missing/invalid `name` or `description`) block; warnings (very short description, an over-long body, a `name` that doesn't match the directory) advise. Fix the errors and weigh the warnings.

## Testing a skill

After a draft, come up with 2-3 **realistic** test prompts — the kind of thing a real user would actually type, with concrete detail (file names, real values, a bit of backstory), not abstract one-liners. Show them to the user: "Here are a few cases I'd like to try — look right?" Then run them.

Fleety has subagents, so the clean way to test is to run each prompt in a fresh `spawn_subagent` that loads the skill (point it at the skill by name; the `cheap` tier is fine for routine cases), so the run doesn't have your authoring context coloring it. Read each subagent's **actual output** (anti-snowball) and review the results with the user qualitatively: show the prompt and what it produced, ask "how does this look — anything you'd change?".

Keep it proportionate. There's no bundled benchmark/eval-viewer machinery here — qualitative review with the user plus a couple of focused runs is the tool. If a skill's output is objectively checkable, you can also write a small check script and run it via `run_command`.

## Improving a skill

This is the heart of the loop. After the user reviews the runs:

1. **Generalise from the feedback.** You're building something meant to run many times across many prompts — don't bolt on fiddly per-example fixes or oppressive constraints. If an issue is stubborn, try a different framing or metaphor rather than another `MUST`.
2. **Keep it lean.** Remove instructions that aren't pulling their weight. Read the run transcripts, not just the final outputs — if the skill is making the agent waste steps, cut the part causing it.
3. **Explain the why** (again) — terse or frustrated feedback still has a real need behind it; understand it and transmit that understanding into the instructions.
4. **Notice repeated work.** If every test run independently wrote a similar helper script or took the same multi-step path, that's a strong signal to bundle it as a `scripts/` tool once and have the skill call it.

Apply improvements with `skill_edit_file`, rerun the prompts, review again. Stop when the user's happy, the feedback's all positive, or you've stopped making meaningful progress.

## Relationship to the learning loop

After a complex task, Fleety's learning loop may prompt you to persist what you learned. This skill is the "how": a reusable **procedure** becomes an authored skill (with a `scripts/` tool when a step is better as code); durable **facts** about the user or project go to memory or the wiki instead; one-off, conversation-only details get saved nowhere. Don't save what code or git already makes obvious. When updating an existing authored skill, keep its name and refine in place rather than spawning a near-duplicate.
