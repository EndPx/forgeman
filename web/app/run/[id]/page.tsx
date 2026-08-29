import { loadEvents, loadRun, statusLabel, statusReason } from "@/lib/runs";

export const dynamic = "force-dynamic";

export default async function RunPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = await params;
  const run = await loadRun(id);

  if (!run) {
    return (
      <main>
        <div className="panel">
          <h2>Run not found</h2>
          <p className="muted">
            No record for <code>{id}</code>. Try the run list.
          </p>
        </div>
      </main>
    );
  }

  const badge = statusLabel(run.status);
  const reason = statusReason(run.status);
  const events = await loadEvents(id);

  const firstTests = run.iterations?.find((it) => it.tests)?.tests;
  const lastTests = [...(run.iterations ?? [])]
    .reverse()
    .find((it) => it.tests)?.tests;

  const stageLabel: Record<string, string> = {
    inspect: "inspect",
    analyze: "analyze",
    plan: "plan",
    implement: "implement",
    test: "test",
    diagnose: "diagnose",
    improve: "improve",
    verify: "verify",
    report: "report",
  };

  return (
    <main>
      <div className="panel">
        <h2>
          <span className="badge {badge.tone}"></span>
          <span className={`badge ${badge.tone}`}>{badge.text}</span>{" "}
          {run.id}
        </h2>
        <p>
          {run.task?.description}
          {reason ? <span className="bad"> — {reason}</span> : null}
        </p>
        <div className="metrics">
          <div className="metric">
            <div className="label">Tests</div>
            <div className="value">
              {firstTests && lastTests && firstTests.passed !== lastTests.passed
                ? `${firstTests.passed} → ${lastTests.passed}/${lastTests.total}`
                : lastTests
                  ? `${lastTests.passed}/${lastTests.total}`
                  : "—"}
            </div>
          </div>
          <div className="metric">
            <div className="label">Iterations</div>
            <div className="value">{run.iterations?.length ?? 0}</div>
          </div>
          <div className="metric">
            <div className="label">Tool calls</div>
            <div className="value">{run.tool_executions?.length ?? 0}</div>
          </div>
          <div className="metric">
            <div className="label">Cost</div>
            <div className="value">${(run.total_cost_usd ?? 0).toFixed(4)}</div>
          </div>
          {run.baseline_commit ? (
            <div className="metric">
              <div className="label">Baseline</div>
              <div className="value commit">{run.baseline_commit}</div>
            </div>
          ) : null}
        </div>
      </div>

      <div className="panel">
        <h2>Iterations &amp; Decision Trace</h2>
        {(run.iterations ?? []).length === 0 ? (
          <p className="muted">No iterations recorded.</p>
        ) : (
          (run.iterations ?? []).map((iteration) => (
            <div className="iteration" key={iteration.index}>
              <div className="head">
                <strong>#{iteration.index}</strong>
                {iteration.tests ? (
                  <span className={iteration.tests.failed > 0 ? "bad" : "ok"}>
                    {iteration.tests.passed}/{iteration.tests.total} tests
                  </span>
                ) : (
                  <span className="muted">no test data</span>
                )}
                {iteration.git_commit ? (
                  <span className="commit">⌾ {iteration.git_commit}</span>
                ) : null}
              </div>
              {iteration.stage_results?.map((stageResult, key) => (
                <div key={key} className="muted">
                  {stageLabel[stageResult.stage] ?? stageResult.stage} [
                  {stageResult.status}] {stageResult.detail}
                </div>
              ))}
              {iteration.failures?.map((failure, key) => (
                <div key={key}>
                  <span className="bad">✗ {failure.stage}</span>{" "}
                  {failure.message}
                  {failure.root_cause ? (
                    <div>
                      <strong>root cause:</strong> {failure.root_cause}
                      {typeof failure.confidence === "number"
                        ? ` (${Math.round(failure.confidence * 100)}%)`
                        : ""}
                    </div>
                  ) : null}
                  {failure.recommended_action ? (
                    <div>
                      <strong>next action:</strong> {failure.recommended_action}
                    </div>
                  ) : null}
                </div>
              ))}
            </div>
          ))
        )}
      </div>

      <div className="panel">
        <h2>Event Log (tail)</h2>
        {events.length === 0 ? (
          <p className="muted">No events recorded.</p>
        ) : (
          <div className="events">
            {events.map((event, key) => (
              <div key={key}>
                [{event.timestamp}] {event.event}
              </div>
            ))}
          </div>
        )}
      </div>
    </main>
  );
}
