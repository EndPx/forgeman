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
spending an iteration on tests.

## Round 2 — closing the loop on the challenging case

The challenging case produced a concrete fix, which the protocol then
re-measured (baseline unchanged; ForgeMan gained one capability):

| Fix | What it does |
|---|---|
| **Structural sanity check** | After applying edits, before spending a test iteration: node local imports must resolve to files that exist after the edit set; edited `.py` files must `py_compile`. Violations reject the edit set with a precise reason |
| **Feedback injection** | The rejection reason is stored and appended to the retried stage's prompt, so the model sees exactly what was rejected and why |
| **Prompt hardening** | Coder/improver: "edit existing files IN PLACE — do not split existing code into new files" |
| **Longer 429 backoff** | 5s/15s/30s exponential wait in the provider (free tier was heavily overloaded during round 2) |

Round 2 results (same protocol, fresh baselines):

| Case | ForgeMan round 1 (final state) | ForgeMan round 2 (final state) |
|---|---|---|
| `flawed-js` | exhausted ×2; **tree unimportable** (0 tests could run) | exhausted, but **2/3 preserved — the sanity check rejected the broken edit sets** before they did damage; 3 of 6 failures were 429 storms |
| `flawed-py` | exhausted 2/3 | exhausted; an edit introduced 1 runtime error (errors=1) and diagnose itself hit a 429 |
| `flawed-api` | VERIFIED 5/5 | kept (not re-run) |

**Takeaway:** the sanity check did exactly what it was built for — the
catastrophic regression (unimportable tree) disappeared — but the free-tier
model still could not produce a fully passing suite for the js/py cases, and
persistent 429s consumed whole iterations. Reliability is currently bounded by
the model tier, not by the loop; the loop's job — make every failure visible,
bounded, and diagnosable — held in all cases.

## Round 3 — performance gates

The task text always said *"fix the API performance issue"* — but the suites
only gated correctness, so nothing forced the perf fix. Round 3 added
**performance gates** to all three suites:

- `flawed-api`: deterministic parse-count gate — 20 lookups over 50 rows must
  not re-parse more than 50 rows (the naive N+1 parses 1000)
- `flawed-js`: timing gate — 1.5M-item dedup must finish < 500ms (naive O(n²)
  measures ~1010ms; a Set-based fix ~60ms)
- `flawed-py`: timing gate — 4000-item top-k must finish < 1s (naive O(n²)
  measures ~1.8s)

Round 3 re-ran `flawed-api`: **exhausted at 2/3** — the perf gate and one
broken test were fixed, one regression remained, and the free-tier model ran
out of iterations. The higher, more honest bar is simply harder for this
model tier; the round-1 verified run (5/5 without the perf gate) still stands
for the 5-test suite version, clearly labeled per suite version in the run
records.

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
