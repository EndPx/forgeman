# Getting Started with ForgeMan

From zero to your first verified engineering run in ~10 minutes.

## 1. Prerequisites

| Tool | Needed for | Check |
|---|---|---|
| [Rust 1.85+](https://rustup.rs) | building the binary | `cargo --version` |
| [Node.js 18+](https://nodejs.org) | building the dashboard (dev-time only) | `node --version` |
| [Git](https://git-scm.com) | checkpoints & diffs | `git --version` |
| A free [Z.AI API key](https://z.ai) | the LLM | — |
| [Docker](https://docker.com) *(optional)* | sandboxed execution | `docker info` |

## 2. Build (one command)

```bash
git clone https://github.com/EndPx/forgeman
cd forgeman
npm run build-all
```

This builds the web dashboard, embeds it into the binary, and compiles
`target/release/forgeman(.exe)`. Put that file on your `PATH`, or use
`cargo install --path .`.

## 3. Add your API key

```bash
cp .env.example .env
# edit .env and set:
#   ZAI_API_KEY=<your key from z.ai>
```

Every user brings **their own key** — ForgeMan never ships or commits keys.
`.env` is gitignored; verify with `git status` (it must not appear).

## 4. First run

```bash
# try it on the built-in demo (a deliberately broken repository):
bash scripts/demo.sh
cd examples/flawed-api
forgeman run "Fix the API performance issue and make the failing tests pass"
```

You will see the loop live:

```text
▸ inspect → analyze → plan → implement → test → diagnose → improve → verify
```

When it finishes, inspect the evidence:

```bash
forgeman report    # baseline → final tests, root causes, checkpoints
forgeman diff      # the exact change the agent made
forgeman dashboard # visual UI with the decision trace
```

## 5. Use it on your own project

```bash
cd ~/projects/your-project   # any git repo with cargo/npm/pytest tests
forgeman init                # optional: writes .forgeman/config.toml
forgeman run "Add pagination to the users endpoint"
```

Tips:

- Commit (or stash) your work before a run — ForgeMan creates its own
  checkpoints, so you can always `git diff` or revert its changes.
- Task phrasing matters: describe the *problem and what proves it's solved*
  ("make the failing tests pass", "reduce latency of X") rather than
  prescribing code.
- The run is bounded by `execution.max_iterations`, `timeout_minutes`, and
  `budget.max_cost_usd` in `.forgeman/config.toml`.

## Troubleshooting

| Symptom | Meaning / fix |
|---|---|
| `missing API key for provider 'zai'` | No `ZAI_API_KEY` in `.env` or environment |
| `provider returned error 429` | Free-tier rate limit — ForgeMan retries automatically; wait a moment and re-run |
| `model exhausted max_tokens on reasoning` | The model spent the budget thinking; already retried — re-run usually clears it |
| `required stages not registered yet` | You are on an old binary — rebuild |
| No git checkpoints | Target repo needs `git config user.email` / `user.name`, and must be a git repo |
| Dashboard shows "Cannot reach the ForgeMan API" | Open it via `forgeman dashboard`, not by double-clicking a file — the UI needs the API |

## Where the evidence lives

```text
<repo>/.forgeman/
├── runs/<run_id>/run.json        # full run record (config snapshot included)
├── runs/<run_id>/events.jsonl    # every event, streamed live
├── reports/<run_id>.md           # human-readable engineering report
└── profile.json                  # repository intelligence profile
```
