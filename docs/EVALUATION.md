# ForgeMan — Evaluation

Fair comparison between the **baseline** (a naive one-shot LLM call, the way a
developer uses a chat model today) and the **ForgeMan closed-loop agent**, on
the same tasks, the same repositories, and the same model
(Z.AI `glm-4.7-flash`, free tier).

## Primary metric

**Tests passing out of the repository's own suite** — the repo is the only
judge; no agent self-assessment counts. A case counts as *solved* only when
the suite passes (exit code 0).

## Protocol (applied identically to both sides)

- Every attempt starts from the same baseline commit of the target repository.
- Baseline: `node scripts/baseline.mjs [case]` — one direct prompt with the
  repo files, JSON edits applied by a 30-line script, then the repo's test
  command. Multiple attempts recorded, because a human would re-prompt.
- ForgeMan: `forgeman run "<task>" --max-iterations 3 --timeout-minutes 15`.
  Up to 2 runs per case, because a human would also re-run the agent.
- Free-tier 429s are retried with bounded backoff on both sides
  (the naive baseline *without* retries lost attempts to 429 — recorded as such).

## Cases

| Case | Repository | Planted defects | Test suite |
|---|---|---|---|
| `flawed-api` | Rust (cargo) | broken build, 2 failing tests, full-scan-per-lookup hot path | `cargo test` (5 tests) |
| `flawed-js` | Node (npm) | inverted discount math, O(n²) dedup, 1 failing test | `node --test` TAP (3 tests) |
| `flawed-py` | Python (unittest) | wrong separator in slugify, O(n²) top-k, 1 failing test | `python -m unittest` (3 tests) |

## Results

| Case | Baseline (best of attempts) | ForgeMan (best of 2 runs) | Winner |
|---|---|---|---|
| `flawed-api` (Rust) | 2/5 tests, never passed (3 attempts: 2× 429, 1× applied edit left 1 test failing) | **5/5 — VERIFIED** in 3 iterations, 2 compile errors diagnosed with root cause + confidence | **ForgeMan** |
| `flawed-js` (Node) | 2/3 tests, never passed (2 attempts: 2× 429; earlier attempts applied edits but left the suite failing) | **exhausted twice** — the model kept splitting code into a new module without writing it; final tree did not even load | Baseline (less damage) |
| `flawed-py` (Python) | 2/3 tests, never passed (2 attempts: 1× no parsable JSON → 0 edits; 1× edit broke imports → 10 test errors) | exhausted; final state 2/3 — no regression, no fix | Tie |
| **Cases solved** | **0 / 3** | **1 / 3** (the hardest: compile errors + 2 broken tests) | |

Raw data: [`baseline-results.json`](baseline-results.json) (baseline) and
`.forgeman/runs/*/run.json` (ForgeMan, per repository).

## What the numbers mean

1. **Where ForgeMan clearly wins** is the case that needs *diagnosis*: the
   Rust repo does not even compile, and each fix exposed the next error. The
   loop (implement → test → diagnose → improve, with the failure analysis
   persisted across iterations) walked through two cascading compile errors to
   a fully passing suite. The one-shot baseline never got the suite green
   anywhere.
2. **Where ForgeMan loses is model variance, not loop design**: with the free
   glm-4.7-flash tier the coder twice tried to split `report.js` into a new
   module and never wrote the new file. The loop made the failure *visible*
   (exhausted, honest exit code) but did not rescue it within 3 iterations.
   With a stronger model this is the first thing we would re-measure.
3. **The baseline is not a strawman**: it applied real edits, and in one case
   its outcome (2/3) was better than ForgeMan's final state on that case. We
   report it as-is.

## Challenging case

`flawed-js` is the challenging case: two ForgeMan runs exhausted and the final
tree was left *unimportable*. What it revealed: the improve stage received an
accurate diagnosis (module `./pricing.js` is missing) but the free-tier model
repeatedly produced an edit set that rewrote the importer without writing the
module. Lesson (now in the changelog): the loop needs a **structural edit
check** (every referenced local module must exist after applying edits) before
spending an iteration on tests. That check is the first item on our roadmap —
and it can only be discovered by an evaluation like this one.

## Reproduce

```bash
# prerequisites: rust, node, python, git; ZAI_API_KEY in .env
npm run build-all

# 1. init the eval repos (fresh git baselines)
bash scripts/init-eval-repos.sh

# 2. baseline (one-shot prompt per attempt, ~1–2 min per attempt)
node scripts/baseline.mjs

# 3. ForgeMan (full loop, ~2–6 min per case)
cd examples/flawed-api
../target/release/forgeman run "Fix the API performance issue and make the failing tests pass" --max-iterations 3 --timeout-minutes 15
cd ../flawed-js
../target/release/forgeman run "Fix the discount bug and the slow report generation, and make the failing tests pass" --max-iterations 3 --timeout-minutes 15
cd ../flawed-py
../target/release/forgeman run "Fix the slugify bug and the slow top_titles function, and make the failing tests pass" --max-iterations 3 --timeout-minutes 15

# 4. inspect the evidence
forgeman report && forgeman diff && forgeman dashboard
```

Approximate cost of the entire evaluation: **$0.00** (free-tier glm-4.7-flash),
wall time ≈ 45 minutes including reruns.
