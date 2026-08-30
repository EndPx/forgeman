# ForgeMan

> **Autonomous Software Engineering Agent** — *AI that engineers, not just codes.*

ForgeMan is a closed-loop software engineering system. It receives a real engineering problem, understands the repository, builds an engineering plan, implements the solution, runs independent validation, diagnoses failures, iterates, and produces an evidence-backed report proving the solution actually works.

**Core principle:** code generated ≠ problem solved. `VERIFIED` is only claimed when tests, benchmarks, and evaluators prove it.

## Core Loop

```text
UNDERSTAND → INSPECT → PLAN → IMPLEMENT → TEST → OBSERVE → DIAGNOSE
    → (FAIL: IMPROVE / PASS: VERIFY) → REPORT
```

Iteration 0 implements the plan. Iterations 1+ improve from the failure
analyzer's root-cause diagnosis. Every iteration becomes a git checkpoint.

## Quick Start

```bash
# 1. Build
cargo build

# 2. Configure (provider credentials live in .env — see .env.example)
cp .env.example .env   # then add your ZAI_API_KEY
forgeman init          # scaffolds .forgeman/config.toml

# 3. Run the full engineering loop on a repository
cd path/to/your/repo
forgeman run "Fix the authentication expiration bug"

# 4. Inspect the evidence
forgeman report        # baseline vs final, failures, root causes, checkpoints
forgeman diff          # what changed since baseline (git)
forgeman history       # every run and its checkpoints
```

### Standalone commands

| Command | Purpose |
|---|---|
| `forgeman init` | Scaffold `.forgeman/config.toml` |
| `forgeman inspect` | Repository intelligence profile (language, framework, deps, risk areas) |
| `forgeman analyze "task"` | LLM task analysis against the repository |
| `forgeman plan "task"` | LLM implementation plan (steps, validation criteria, rollback) |
| `forgeman run "task"` / `solve` | The full engineering loop |
| `forgeman test` | Run the repository's validation suite (cargo / npm / pytest) |
| `forgeman report` / `diff` / `history` | Evidence and reporting |

## Killer Demo (spec §42)

```bash
bash scripts/demo.sh                       # init examples/flawed-api as a git repo
cd examples/flawed-api
forgeman run "Fix the API performance issue and make the failing tests pass"
forgeman report && forgeman diff
```

`examples/flawed-api` is deliberately flawed: a full-scan-per-lookup (N+1)
hot path, deep clones in the report path, and two broken tests. Watch
ForgeMan diagnose, fix, iterate, and verify.

## Web Dashboard

The dashboard is **embedded in the binary** — no Node needed at runtime.

```bash
# one command builds everything (dashboard + release binary):
npm run build-all

# then serve it from any repository that has ForgeMan runs:
forgeman dashboard            # http://127.0.0.1:3777
```

Shows every run: status, baseline vs final tests, iterations with git
checkpoints, decision trace (evidence → root cause → next action → result),
and the raw event log. For UI development use `npm run dev --prefix web`
(then `npm run build-web` to re-embed).

## Configuration (`.forgeman/config.toml`)

```toml
[agent]
provider = "zai"          # zai | anthropic | openai
model = "glm-4.7-flash"   # free Z.AI flash tier by default

[execution]
max_iterations = 5
timeout_minutes = 20
max_stage_attempts = 3

[sandbox]
enabled = false            # true → Docker isolation (network none, 1g, 1 cpu, pids 128)
network = "restricted"

[budget]
max_cost_usd = 5.0
```

Credentials are read from `.env` (`ZAI_API_KEY`, optional `ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`). `.env` is gitignored and must never be committed.

## Architecture

See [docs/architecture.md](docs/architecture.md). Highlights:

- **Stage contract** — every loop step is a `Stage` producing artifacts into
  `RunContext` (`repository.profile`, `task.analysis`, `plan`,
  `tests.result`, `failure.analysis`, …).
- **Independent evaluation** — the diagnoser judges only from evidence; the
  verifier asserts the stop condition; `VERIFIED` requires passing tests.
- **Audited tools** — file edits are path-confined to the repository and
  logged (`tool.started`/`tool.completed` + `ToolExecution` records).
- **Reproducibility** — config snapshot + event JSONL + git checkpoints per
  iteration under `.forgeman/runs/<run_id>/`.

## Status

All phases complete:

- [x] Phase 1 — CLI + core orchestration engine
- [x] Phase 2 — Repository inspector
- [x] Phase 3 — LLM provider abstraction (Z.AI / Anthropic / OpenAI)
- [x] Phase 4 — Analyzer + Planner (LLM-backed)
- [x] Phase 5 — Audited tool layer + Coder stage
- [x] Phase 6 — Test Runner (cargo / npm / pytest)
- [x] Phase 7 — Failure Analyzer (independent root-cause diagnosis)
- [x] Phase 8 — Iterative Improvement Engine + Verify gate
- [x] Phase 9 — Git checkpoints, diff/history, engineering report
- [x] Phase 10 — Sandbox (Docker isolation with resource limits)
- [x] Phase 11 — Web dashboard
- [x] Phase 12 — Demo scenario + documentation

## Build

```bash
npm run build-all    # everything: dashboard + release binary (one command)
cargo test           # core test suite
```
