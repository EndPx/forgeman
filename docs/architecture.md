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

## Module Map (Phase 1)

```text
src/
├── main.rs               — CLI entry, dispatch, run summary / report printing
├── cli.rs                — clap command surface (init, inspect, analyze, plan,
│                           run, solve, test, improve, report)
├── config.rs             — forgeman.toml loading, spec defaults, scaffolding
└── core/
    ├── model.rs          — Task, Run, Iteration, StageResult, TestSummary,
    │                       FailureRecord, RunStatus (serde-serializable)
    ├── events.rs         — event types (dotted names), ConsoleSink + JsonlSink
    ├── store.rs          — .forgeman/runs/<run_id>/{run.json,events.jsonl}
    ├── stage.rs          — Stage trait, StageOutput, StageError, RunContext
    └── orchestrator.rs   — pipeline engine: preamble → iteration loop →
                            report, stop conditions, bounded retries
```

## Orchestration Flow

1. **Preamble** — `inspect` → `analyze` → `plan` run once before iterating.
   Missing required stages abort the run with an informative reason (honest,
   never a fake success).
2. **Iteration loop** — `implement` → `test` → (`diagnose` when tests fail)
   → stop-condition check → `improve` → next iteration.
3. **Stop conditions** (spec §18/25):
   - all tests pass and no critical regression → `VERIFIED`
   - `max_iterations` reached → `EXHAUSTED`
   - wall-clock deadline passed → `TIMED OUT`
   - `total_cost_usd >= budget.max_cost_usd` → `BUDGET EXCEEDED`
4. **Escalation** — a stage retries up to `execution.max_stage_attempts`
   (default 3); `Blocked` errors are non-retryable. After exhaustion the run
   is marked `FAILED` with the reason. No infinite retry.
5. **Report** — optional stage; every run is persisted as evidence either way.

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
`repository.profile`, `task.analysis`, `plan`, `tests.result`) and publish
their own outputs for downstream stages. The orchestrator is generic over
how stages work; real stage implementations land in Phases 2–8:

| Phase | Stage(s)           | Artifact produced        |
|-------|--------------------|--------------------------|
| 2     | inspect            | `repository.profile`     |
| 4     | analyze, plan      | `task.analysis`, `plan`  |
| 5     | implement          | `implementation.diff`    |
| 6     | test               | `tests.result`           |
| 7     | diagnose           | `failure.analysis`       |
| 8     | improve, verify    | `fix.plan`               |
| 9     | report             | `engineering-report.md`  |

## Principles (non-negotiable)

- Code generated ≠ problem solved. `VERIFIED` only from evidence.
- The agent that wrote the code is never its only evaluator.
- Every iteration is a reproducible checkpoint (config snapshot + events).
- No infinite loops: iterations, attempts, budget, and timeout are all bounded.
- Configuration is externalized (`forgeman.toml`), never hardcoded.
