import Link from "next/link";
import { listRuns, statusLabel, statusReason } from "@/lib/runs";

export const dynamic = "force-dynamic";

function formatWhen(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toISOString().replace("T", " ").slice(0, 16) + " UTC";
}

export default async function RunsPage() {
  const runs = await listRuns();

  return (
    <main>
      <div className="panel">
        <h2>Engineering Runs</h2>
        {runs.length === 0 ? (
          <div className="empty">
            No runs yet. Start the loop:
            <br />
            <code>forgeman run &quot;Fix the authentication bug&quot;</code>
            <br />
            <br />
            Run records appear under <code>.forgeman/runs/</code> and are read
            live by this dashboard.
          </div>
        ) : (
          <table className="list">
            <thead>
              <tr>
                <th>Run</th>
                <th>Task</th>
                <th>Status</th>
                <th>Iterations</th>
                <th>Cost</th>
                <th>Started</th>
              </tr>
            </thead>
            <tbody>
              {runs.map((run) => {
                const badge = statusLabel(run.status);
                const reason = statusReason(run.status);
                return (
                  <tr key={run.id}>
                    <td>
                      <Link href={`/run/${run.id}`}>{run.id}</Link>
                    </td>
                    <td>{run.task?.description ?? "—"}</td>
                    <td>
                      <span className={`badge ${badge.tone}`} title={reason ?? ""}>
                        {badge.text}
                      </span>
                    </td>
                    <td>{run.iterations?.length ?? 0}</td>
                    <td>${(run.total_cost_usd ?? 0).toFixed(4)}</td>
                    <td className="muted">{formatWhen(run.started_at)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </main>
  );
}
