# Breaking down large work

Agents asked to break a project down tend to write one task per step: hundreds of tiny tasks that
are really the checklists of a few real ones. Workgraph works best with a **ladder of
milestones**: each task leaves the system working in a new state a check can prove, and the next
task builds on it. Only the next milestone is broken into child tasks. Later milestones stay as
one task each until the work reaches them.

## The orchestrator skill

[`docs/skills/milestone-breakdown/SKILL.md`](skills/milestone-breakdown/SKILL.md) is a Claude Code
skill that teaches an orchestrator this procedure: what counts as a task, how to write the ladder,
when to decompose, what to do with things noticed mid-task, and how to write the ladder for
`task import-breakdown`. Install it for every project by copying the folder:

```sh
mkdir -p ~/.claude/skills
cp -r docs/skills/milestone-breakdown ~/.claude/skills/
```

or for one project by copying it into that project's `.claude/skills/`.

## Settings that enforce it

The MCP `task_create` description tells agents the same rules. The settings below enforce them in
tooling rather than leaving them to the prompt (see [Configuration](configuration.md)):

```toml
[tasks]
max_open_children = 8        # refuse a create under a parent that already has 8 open children
duplicate_check = "refuse"   # refuse a create that matches an open task
intake_proposed = true       # hold agent-created tasks as proposed until a human admits them
require_acceptance_criteria = true
```

`max_open_children` covers `task create` from the CLI, HTTP and MCP. `task import-breakdown` is a
deliberate bulk import of a reviewed document and is not limited.
