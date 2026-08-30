/** Dashboard data — fetched live from the ForgeMan binary's API. */

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
  /** Present on in-progress runs read from the live event log. */
  iterations_count?: number;
  current_stage?: string;
};

export type Event = { timestamp: string; event: string } & Record<string, unknown>;

async function getJson<T>(url: string): Promise<T | null> {
  try {
    const response = await fetch(url, { cache: "no-store" });
    if (!response.ok) return null;
    return (await response.json()) as T;
  } catch {
    return null;
  }
}

export function listRuns(): Promise<Run[] | null> {
  return getJson<Run[]>("/api/runs");
}

export function loadRun(id: string): Promise<Run | null> {
  return getJson<Run>(`/api/runs/${encodeURIComponent(id)}`);
}

export function loadEvents(id: string): Promise<Event[] | null> {
  return getJson<Event[]>(`/api/runs/${encodeURIComponent(id)}/events`);
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
