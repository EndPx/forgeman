# ForgeMan Architecture

> AI that engineers, not just codes.

ForgeMan is a closed-loop software engineering system: it understands a repository,
plans the work, implements it, validates independently, diagnoses failures,
iterates, and reports evidence.

## Core Loop

```text
UNDERSTAND → INSPECT → PLAN → IMPLEMENT → TEST → OBSERVE → DIAGNOSE
    → (FAIL: IMPROVE / PASS: VERIFY) → REPORT
```

## Module Map (final)

```text
src/
├── main.rs               — CLI entry, dispatch, summaries, report/diff/history
├── cli.rs                — clap command surface
├── config.rs             — forgeman.toml loading, spec defaults, scaffolding
├── env.rs                — minimal .env loader (credentials never committed)
├── git.rs                — checkpoint commits, diffs, history (spec §22)
├── providers/            — LLM abstraction (spec §35)
│   ├── mod.rs            — AgentProvider trait, Prompt/Response, cost model
│   ├── zai.rs            — Z.AI GLM (default, glm-4.7-flash), 429 backoff
│   ├── anthropic.rs      — Anthropic Messages API
│   ├── openai.rs         — OpenAI Chat Completions (compatible endpoints)
│   ├── router.rs         — model-role routing (spec §13)
│   └── test_util.rs      — one-shot localhost HTTP mock (tests only)
├── tools/                — audited, path-confined file/search tools (§36/§37)
├── sandbox/              — Process | Docker isolation with resource limits
├── repository/           — inspector + profile (spec §7–8)
└── core/
    ├── model.rs          — Task, Run, Iteration, ToolExecution, statuses
    ├── events.rs         — dotted event types, console + JSONL sinks
    ├── store.rs          — .forgeman/runs/<run_id>/{run.json,events.jsonl}
    ├── stage.rs          — Stage trait, StageOutput, StageError, RunContext
    └── orchestrator.rs   — pipeline engine: preamble → iteration loop →
                            report, stop conditions, bounded retries, git
                            checkpoints
agents/
├── inspect.rs            — repository explorer        → repository.profile
├── analyze.rs            — task → problem definition  → task.analysis
├── plan.rs               — executable plan            → plan
├── coder.rs              — plan edits (write/delete)  → implementation.changes
├── test_runner.rs        — cargo/npm/pytest runner    → tests.result
├── diagnose.rs           — independent root-cause     → failure.analysis
├── improve.rs            — fix from diagnosis         → improvement.changes
└── verify.rs             — evidence gate              → verification
web/                      — Next.js dashboard reading .forgeman/runs/
examples/flawed-api/      — killer-demo repository (spec §42)
```

## Orchestration Flow

1. **Preamble** — `inspect` → `analyze` → `plan` run once before iterating.
   Missing required stages abort the run with an informative reason (honest,
   never a fake success).
2. **Iteration loop** — iteration 0: `implement` → `test`; iterations 1+:
   `improve` (from the previous `failure.analysis`) → `test`; when tests
   fail, `diagnose` explains why with evidence and confidence.
3. **Checkpoint** — each iteration ends in a git commit (`forgeman: iteration
   N — tests X/Y`) stored on the iteration record; `forgeman diff` shows the
   full change since the run's baseline commit.
4. **Stop conditions** (spec §18/25):
   - all tests pass and no critical regression → `VERIFIED` (after the
     `verify` evidence gate re-asserts them)
   - `max_iterations` reached → `EXHAUSTED`
   - wall-clock deadline passed → `TIMED OUT`
   - `total_cost_usd >= budget.max_cost_usd` → `BUDGET EXCEEDED`
5. **Escalation** — a stage retries up to `execution.max_stage_attempts`
   (default 3); `Blocked` errors are non-retryable. After exhaustion the run
   is marked `FAILED` with the reason. No infinite retry.
6. **Report** — `forgeman report` assembles baseline vs final tests,
   checkpoints, failure root causes, tool usage, and cost into a markdown
   report under `.forgeman/reports/`.

## Observability

Every stage start/completion, failure, iteration boundary, and run outcome is
emitted as a structured event:

- console (human-readable, stderr), and
- `<repo>/.forgeman/runs/<run_id>/events.jsonl` (machine-readable, stable
  dotted event names like `stage.started`, `failure.detected`, `run.completed`).

Run records (`run.json`) snapshot the task and config for reproducibility.

## Stage Contract

```rust
pub trait Stage: Send + Sync {
    fn name(&self) -> StageName;
    fn execute<'a>(&'a self, ctx: &'a mut RunContext) -> StageFuture<'a>;
}
```

Stages read upstream artifacts from `RunContext.artifacts` (e.g.
`repository.profile`, `task.analysis`, `plan`, `tests.result`,
`failure.analysis`) and publish their own outputs for downstream stages.
The orchestrator is generic over how stages work:

| Stage     | Artifact produced            |
|-----------|------------------------------|
| inspect   | `repository.profile`         |
| analyze   | `task.analysis`              |
| plan      | `plan`                       |
| implement | `implementation.changes`     |
| test      | `tests.result`, `tests.output` |
| diagnose  | `failure.analysis`           |
| improve   | `improvement.changes`        |
| verify    | `verification`               |

## Principles (non-negotiable)

- Code generated ≠ problem solved. `VERIFIED` only from evidence.
- The agent that wrote the code is never its only evaluator.
- Every iteration is a reproducible checkpoint (config snapshot + events + git commit).
- No infinite loops: iterations, attempts, budget, and timeout are all bounded.
- Configuration is externalized (`forgeman.toml`), never hardcoded.
