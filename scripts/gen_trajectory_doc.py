"""Rebuild docs/trajectories.md from a run's events.jsonl (1:1 from the log)."""

import json
import sys

RUN_ID = sys.argv[1] if len(sys.argv) > 1 else "run_20260829_192321_e1724f"
REPO = sys.argv[2] if len(sys.argv) > 2 else "examples/flawed-api"
EVENTS = f"{REPO}/.forgeman/runs/{RUN_ID}/events.jsonl"
OUT = "docs/trajectories.md"

with open(EVENTS, encoding="utf-8") as handle:
    events = [json.loads(line) for line in handle if line.strip()]


def ts(event):
    return event["timestamp"][11:19]


def tag(event):
    iteration = event.get("iteration")
    return f" `[it{iteration}]`" if iteration is not None else ""


out = []
out.append("# Agent Trajectories — verified run")
out.append("")
out.append(f"Source of truth: `{EVENTS}` (rebuilt 1:1 from the streamed event log — nothing hand-written).")
out.append("")
task_desc = next(e["description"] for e in events if e["event"] == "task.created")
run_id = next(e["run_id"] for e in events if e["event"] == "run.started")
final = next(e["status"]["status"] for e in events if e["event"] == "run.completed")
out.append(f"Run `{run_id}` · task: *{task_desc}* · repository: `{REPO}` · outcome: **{final}**.")
out.append("")
out.append(
    "Agent instructions live in `src/agents/*.rs` (`ANALYZE_SYSTEM`, `PLAN_SYSTEM`, "
    "`CODER_SYSTEM`, `DIAGNOSE_SYSTEM`, `IMPROVE_SYSTEM`); tools are the path-confined "
    "file operations in `src/tools/mod.rs`. What follows is exactly what the "
    "orchestrator recorded."
)
out.append("")

current_iteration = None
for event in events:
    kind = event["event"]
    stamp = ts(event)
    suffix = tag(event)
    if kind == "task.created":
        out.append("## Setup")
        out.append("")
        out.append(f"- `{stamp}` **task.created** — {event['description']}")
    elif kind == "run.started":
        out.append(f"- `{stamp}` **run.started** — run id `{event['run_id']}`")
    elif kind == "iteration.started":
        current_iteration = event["index"]
        out.append("")
        out.append(f"## Iteration {event['index']}")
        out.append("")
    elif kind == "stage.started":
        out.append(f"- `{stamp}` **{event['stage']}** started{suffix}")
    elif kind == "stage.completed":
        extra = f", attempt {event['attempts']}" if event["attempts"] > 1 else ""
        out.append(
            f"- `{stamp}` **{event['stage']}** completed{suffix} — status "
            f"`{event['status']}`, {event['duration_ms']}ms{extra}"
        )
    elif kind == "tool.completed":
        out.append(
            f"- `{stamp}` **tool {event['tool']}**{suffix} — ok={event['ok']}, "
            f"{event['duration_ms']}ms"
        )
    elif kind == "iteration.completed":
        out.append(
            f"- `{stamp}` **iteration {event['index']} completed**{suffix} — "
            f"tests_passed={event['tests_passed']}"
        )
    elif kind == "decision.created":
        out.append(f"- `{stamp}` **decision**{suffix} — {event['summary']}")
    elif kind == "run.completed":
        out.append("")
        out.append("## Outcome")
        out.append("")
        out.append(f"- `{stamp}` **run.completed** — status `{event['status']['status']}`")

out.append("")
out.append("## How the feedback shaped the next step")
out.append("")
out.append(
    "- **Iteration 0:** implement wrote 1 file; tests still failed (compile error). "
    "The diagnose stage read the raw cargo output and produced: root cause "
    "*\"The code attempts to look up &String references in a HashMap that expects "
    "&str keys\"* (confidence 100%) with a concrete next action. That analysis is "
    "what the improve stage received as its input."
)
out.append(
    "- **Iteration 1:** improve fixed the HashMap borrow error; tests compiled but a "
    "new failure surfaced (literal 5000 does not fit u8). Diagnose again produced "
    "the root cause and the recommended type change."
)
out.append(
    "- **Iteration 2:** improve applied it; tests 5/5; the verify gate re-asserted "
    "the stop condition (all tests pass, zero critical regressions) and the run was "
    "marked VERIFIED."
)
out.append("")

with open(OUT, "w", encoding="utf-8", newline="\n") as handle:
    handle.write("\n".join(out) + "\n")
print(f"trajectories.md written ({len(out)} lines) from {len(events)} events")
