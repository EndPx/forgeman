# ForgeMan — Improvement Changelog

How the solution evolved from a naive "paste the repo into a chat model" baseline
to a verified, closed-loop engineering agent. Every stage below was actually
built and measured; nothing on this page is aspirational.

> Ground rule 02 statement: **everything in this repository was built during
> the hackathon (August 29–30, 2026)** — no prior codebase, no prior agent
> framework. The baseline script (`scripts/baseline.mjs`) was written for the
> evaluation and represents the "before" state.

## The evaluation setup (applies to every stage)

- **Task:** *"Fix the API performance issue and make the failing tests pass"*
  on `examples/flawed-api` — a repository with a full-scan-per-lookup hot path,
  a memory-bloating report, a deliberately broken build, and two failing tests.
- **Metric:** tests passing out of the repository's own suite (`cargo test`),
  as reported by the repo itself — never by the agent's self-assessment.
- **Same model everywhere:** Z.AI `glm-4.7-flash` (free tier), same API key.
- **Same repository state:** every attempt and run starts from the same
  baseline commit.

## Baseline — one direct prompt, applied by hand

| | |
|---|---|
| What | `scripts/baseline.mjs`: read the repo files, send **one direct prompt** ("fix the failing tests and the performance problem, return JSON edits"), apply the edits, run `cargo test` |
| Why | This is what a developer does with a chat model today; the brief calls it the fair starting point |
| Evidence | `docs/baseline-results.json` |
| Result | **See [docs/EVALUATION.md](EVALUATION.md)** — no attempt reached a passing suite |
| Learning | Established that the bottleneck is not generation, it is *knowing whether the change worked and what to do when it did not* |

## Stage 1 — CLI + core orchestration (Phase 1)

| | |
|---|---|
| What | Bounded loop engine: stages, retries (max 3), iteration cap, timeout, cost budget, escalation instead of infinite retry |
| Evidence | 15 behavior tests (`run_completes_verified_when_tests_pass`, `run_exhausts_at_max_iterations`, `run_fails_after_repeated_stage_failures`, …); commit `360d634` |
| Decision / learning | Kept. Honest failure states (`EXHAUSTED`, `FAILED`, `ABORTED`) became the backbone of every later stage — the engine can never "claim" success, it can only record evidence |

## Stage 2 — Repository inspector (Phase 2)

| | |
|---|---|
| What | Deterministic repo profile (languages, framework, deps, test frameworks, databases, risk areas) published as the `repository.profile` artifact every downstream stage reads |
| Evidence | 6 detection tests incl. live `forgeman inspect` on this repository; commit `664fa8e` |
| Decision / learning | Kept. Better context = better agent behavior (brief §02): the analyzer/planner/coder all condition on the same profile instead of re-walking the tree |

## Stage 3 — LLM provider abstraction (Phase 3)

| | |
|---|---|
| What | `AgentProvider` trait + Z.AI / Anthropic / OpenAI implementations + model-role router + .env credentials; mock-HTTP tests for all providers |
| Evidence | Provider round-trip tests against a local one-shot HTTP mock; **live** call to glm-4.7-flash returned `OK`; commits `9b94c83`, `28f3d86` |
| Decision / learning | Kept. Core hardcodes no vendor; the free-tier Z.AI model made the whole evaluation run at $0.00 |

## Stage 4 — Analyzer + Planner (Phase 4)

| | |
|---|---|
| What | Task → structured problem definition (`task.analysis`), analysis → executable plan with validation criteria (`plan`) |
| Evidence | Live run: analyzer produced goal/components/risks/edge-cases for a real task in 17k output tokens; planner produced 4 concrete steps; commits `28f3d86` |
| Decision / learning | Kept. Tolerant JSON extraction added after observing models wrap JSON in prose/fences |

## Stage 5 — Coder + audited tools (Phase 5)

| | |
|---|---|
| What | File-edit contract (`write` full content / `delete`), applied through path-confined, audit-logged tools; traversal attempts escalate immediately |
| Evidence | Traversal test proves `../escaped.txt` cannot land; `coder_writes_files_and_logs_tools` records 2 `FileWrite` executions; commit `8ba2231` |
| Decision / learning | **Whole-file writes chosen over unified diffs** — completion models produce unappliable diffs far more often than complete files, and full content is deterministic to apply |
| | **Removed experiment:** arbitrary shell tool for the coder — rejected at design time (ground rule 04: consequential actions must stay controlled) |

## Stage 6 — Test Runner (Phase 6)

| | |
|---|---|
| What | Ecosystem detection (cargo/npm/pytest→unittest), bounded command execution with piped-output reader threads, test-count parsing |
| Evidence | `forgeman test` on this repository: 58/58 passed; parser tests for cargo/pytest/node-tap/unittest formats; commit `a293919` |
| Decision / learning | Kept. The repo's own test suite is the only source of truth — the agent never grades itself |

## Stage 7 — Failure Analyzer (Phase 7)

| | |
|---|---|
| What | Independent diagnoser: reads raw test output + the changes just applied, produces classification, evidence, root cause, confidence, recommended action |
| Evidence | `diagnose_publishes_analysis_and_failure_record` — root cause *"Expiration claim parsed but not validated"* attached to the iteration; commit `c3a83b5` |
| Decision / learning | Kept. This is the independence principle: the diagnoser never sees the coder's rationale, only outcomes |

## Stage 8 — Iterative improvement + verify gate (Phase 8)

| | |
|---|---|
| What | Loop restructure: iteration 0 implements, iterations 1+ improve *from the persisted failure analysis*; verify stage re-asserts the stop condition before `VERIFIED` |
| Evidence | First end-to-end run **failed** with `failure.analysis artifact missing` — the artifact reset was wrong; after the fix the same structure produced a VERIFIED run (tests 0 → 5/5); commit `c3a83b5` → `d443f3b` |
| Decision / learning | **Kept, after a real failure.** `failure.analysis` deliberately survives iteration boundaries — the improver must see the previous diagnosis. Identified as the single change that turned exhausted runs into verified ones |

## Stage 9 — Git checkpoints, evidence, reporting (Phase 9)

| | |
|---|---|
| What | Baseline capture, `git add -A` checkpoint per iteration, `forgeman diff/history/report`, markdown engineering report |
| Evidence | `forgeman history` lists checkpoints per run; report shows 0 → 5/5 (+100%); commit `3ec32d6` |
| Decision / learning | Kept. Also taught a test-isolation lesson: orchestrator tests used `repo_root = "."` and a test run **committed real work-in-progress to the developer's repo** — tests now use isolated temp paths (`fix` in `3ec32d6`) |

## Stage 10 — Live-run hardening (post-Phase-12, driven by real failures)

| | |
|---|---|
| What | Three fixes from observed live-run failures: (1) GLM reasoning burned the entire token budget before emitting JSON → `thinking: disabled` for edit stages + 8192-token budget; (2) coder drifted off-scope (wrote a whole new service instead of fixing `lib.rs`) → minimal-change constraints in the system prompt; (3) escalation message overstated attempt counts → honest counts |
| Evidence | Run 4: 3× `model exhausted max_tokens on reasoning` → after fix, run 5 completed `verified`; drift reproduced in run 3 (`Cargo.toml` + tokio additions reverted after hardening) — commits `d443f3b`, `af13159` |
| Decision / learning | Kept. Decode settings are part of the agent contract, not a detail |

## Removed experiments (and what they taught)

| Experiment | Why removed | Lesson |
|---|---|---|
| Ctrl+C `select!` in the dashboard server | `tokio::signal::ctrl_c()` resolves instantly in console-less environments — server exited at startup | Signal handling must be tested in the environment it runs in |
| `watch` shutdown channel | Same immediate-resolve class of bug | Prefer the simplest primitive that cannot misfire |
| Per-iteration clearing of `failure.analysis` | Broke the improve stage — the next iteration *needs* the previous diagnosis | Artifact lifecycle is loop semantics, not housekeeping |
| Unbounded retries (spec'd early) | Never shipped: bounded attempts + escalation shipped instead | Escalation with an honest reason beats a stalled run |
| Fuzzy-diff application | Never shipped: whole-file writes shipped instead | Apply what models produce reliably, not what is theoretically elegant |

## Final state

- `examples/flawed-api`: tests **0 → 5/5**, run `verified`, 3 iterations, 3 git checkpoints, $0.00.
- Baseline (same model, same task, naive single prompt): **no attempt reached a passing suite** — see [EVALUATION.md](EVALUATION.md).
- 70 automated tests, CI green on Ubuntu + Windows.

## Hot take

The hard part of agentic software engineering is not generating code — it is
**deciding what to do next when the evidence contradicts the agent's claims**.
Our biggest reliability wins came from zero new model quality: (1) feed the
agent's own failure analysis back into the next attempt and let it persist
across iterations, and (2) treat decode settings (`thinking: disabled`, token
budget) as part of the output contract for structured edits. The model wrote
the same bad code in every naive attempt; the loop, not the model, is what
made it pass.
