import fs from "fs/promises";
import path from "path";

/** The dashboard reads ForgeMan run records straight from disk. */
const RUNS_DIR = path.join(process.cwd(), "..", ".forgeman", "runs");
const EVENTS_TAIL = 120;

export type RunStatus = { status: string; reason?: string; iterations?: number };

export type StageResult = {
  stage: string;
  status: string;
  attempts: number;
  duration_ms: number;
  detail?: string;
};

export type TestSummary = {
  total: number;
  passed: number;
  failed: number;
  command: string;
};

export type Failure = {
  stage: string;
  message: string;
  root_cause?: string;
  confidence?: number;
  recommended_action?: string;
};

export type Iteration = {
  index: number;
  stage_results: StageResult[];
  tests?: TestSummary | null;
  failures: Failure[];
  git_commit?: string | null;
};

export type ToolExecution = {
  tool: string;
  arguments?: { path?: string; action?: string };
  result: string;
  duration_ms: number;
};

export type Run = {
  id: string;
  task: { description: string; repo_root?: string };
  started_at: string;
  finished_at?: string | null;
  status: RunStatus;
  iterations: Iteration[];
  total_cost_usd: number;
  tool_executions?: ToolExecution[];
  baseline_commit?: string | null;
};

export type Event = { timestamp: string; event: string } & Record<string, unknown>;

async function readJson<T>(file: string): Promise<T | null> {
  try {
    return JSON.parse(await fs.readFile(file, "utf8")) as T;
  } catch {
    return null;
  }
}

export async function listRuns(): Promise<Run[]> {
  let entries: string[];
  try {
    entries = await fs.readdir(RUNS_DIR);
  } catch {
    return [];
  }
  const runs = await Promise.all(
    entries
      .filter((entry) => entry.startsWith("run_"))
      .sort()
      .reverse()
      .map(async (id) => readJson<Run>(path.join(RUNS_DIR, id, "run.json"))),
  );
  return runs.filter((run): run is Run => run !== null);
}

export async function loadRun(id: string): Promise<Run | null> {
  if (!id.startsWith("run_")) return null;
  return readJson<Run>(path.join(RUNS_DIR, id, "run.json"));
}

export async function loadEvents(id: string): Promise<Event[]> {
  try {
    const raw = await fs.readFile(path.join(RUNS_DIR, id, "events.jsonl"), "utf8");
    const events = raw
      .split("\n")
      .filter((line) => line.trim().length > 0)
      .map((line) => {
        try {
          return JSON.parse(line) as Event;
        } catch {
          return null;
        }
      })
      .filter((event): event is Event => event !== null);
    return events.slice(-EVENTS_TAIL);
  } catch {
    return [];
  }
}

export function statusLabel(status: RunStatus): { text: string; tone: string } {
  const kind = status.status;
  switch (kind) {
    case "verified":
      return { text: "VERIFIED", tone: "ok" };
    case "running":
      return { text: "RUNNING", tone: "run" };
    case "failed":
      return { text: "FAILED", tone: "bad" };
    case "aborted":
      return { text: "ABORTED", tone: "warn" };
    case "exhausted":
      return { text: "EXHAUSTED", tone: "warn" };
    case "timed_out":
      return { text: "TIMED OUT", tone: "bad" };
    case "budget_exceeded":
      return { text: "BUDGET EXCEEDED", tone: "bad" };
    default:
      return { text: kind.toUpperCase(), tone: "warn" };
  }
}

export function statusReason(status: RunStatus): string | null {
  if ("reason" in status && typeof status.reason === "string") {
    return status.reason;
  }
  return null;
}
