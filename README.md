# ForgeMan

> **Autonomous Software Engineering Agent** — *AI that engineers, not just codes.*

ForgeMan is a closed-loop software engineering system. It receives a real engineering problem, understands the repository, builds an engineering plan, implements the solution, runs independent validation, diagnoses failures, iterates, and produces an evidence-backed report proving the solution actually works.

**Core principle:** code generated ≠ problem solved. `VERIFIED` is only claimed when tests, benchmarks, and evaluators prove it.

## Core Loop

```text
UNDERSTAND → INSPECT → PLAN → IMPLEMENT → TEST → OBSERVE → DIAGNOSE
    → (FAIL: IMPROVE / PASS: VERIFY) → REPORT
```

## Status

Phase roadmap (see [docs/architecture.md](docs/architecture.md)):

- [x] **Phase 1** — CLI + core orchestration engine (config, domain model,
      event pipeline, run store, orchestrator with stop conditions and
      escalation, behavior tests)
- [ ] Phase 2 — Repository inspector
- [ ] Phase 3 — LLM provider abstraction
- [ ] Phase 4 — Analyzer + Planner
- [ ] Phase 5 — Coder + tool execution
- [ ] Phase 6 — Test Runner
- [ ] Phase 7 — Failure Analyzer
- [ ] Phase 8 — Iteration engine (end-to-end improve loop)
- [ ] Phase 9 — Git checkpoints + reporting
- [ ] Phase 10 — Sandbox
- [ ] Phase 11 — Web dashboard
- [ ] Phase 12 — Demo scenario + docs polish

## Build

```bash
cargo build
cargo test
cargo run -- solve "Fix the authentication expiration bug"
```
