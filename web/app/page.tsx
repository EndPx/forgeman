"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { listRuns, statusLabel, statusReason, Run } from "@/lib/runs";

export default function RunsPage() {
  const [runs, setRuns] = useState<Run[] | null>(null);
  const [offline, setOffline] = useState(false);

  useEffect(() => {
    listRuns().then((data) => {
      if (data === null) {
        setOffline(true);
      } else {
        setRuns(data);
      }
    });
  }, []);

  return (
    <main>
      <div className="panel">
        <h2>Engineering Runs</h2>
        {offline ? (
          <div className="empty">
            Cannot reach the ForgeMan API.
            <br />
            Start it with <code>forgeman dashboard</code> inside your
            repository.
          </div>
        ) : runs === null ? (
          <div className="empty">Loading…</div>
        ) : runs.length === 0 ? (
          <div className="empty">
            No runs yet. Start the loop:
            <br />
            <code>forgeman run &quot;Fix the authentication bug&quot;</code>
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
                      <Link href={`/run/?id=${encodeURIComponent(run.id)}`}>
                        {run.id}
                      </Link>
                    </td>
                    <td>{run.task?.description ?? "—"}</td>
                    <td>
                      <span className={`badge ${badge.tone}`} title={reason ?? ""}>
                        {badge.text}
                      </span>
                    </td>
                    <td>{run.iterations?.length ?? run.iterations_count ?? 0}{run.current_stage ? <span className="muted"> · {run.current_stage}…</span> : null}</td>
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

function formatWhen(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toISOString().replace("T", " ").slice(0, 16) + " UTC";
}
