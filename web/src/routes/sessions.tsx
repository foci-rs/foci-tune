import { createRoute, Link } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { getSessions, type SessionView } from "@/api";
import { Card } from "@/components/ui";
import { useFeed } from "@/feed";
import { sessionLabel, sortSessions, summarizeSession } from "@/sessions";
import { formatNumber, stateLabel } from "@/tune";
import { Route as rootRoute } from "./__root";

export const Route = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sessions",
  component: SessionsPage,
});

function SessionsPage() {
  const { listen } = useFeed();
  const [sessions, setSessions] = useState<SessionView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);

  const refresh = useCallback(() => {
    const mine = ++generation.current;
    getSessions().then(
      (v) => {
        if (mine !== generation.current) return;
        setSessions(sortSessions(v));
        setError(null);
      },
      (e: unknown) => {
        if (mine === generation.current) setError(e instanceof Error ? e.message : "request failed");
      },
    );
  }, []);

  useEffect(refresh, [refresh]);
  useEffect(
    () =>
      listen((m) => {
        if (m.type === "run_state" || m.type === "run_result" || m.type === "resync") refresh();
      }),
    [listen, refresh],
  );

  return (
    <main className="mx-auto max-w-6xl space-y-4 px-4 py-6">
      <Card label="Sessions">
        {error !== null && <p className="text-sm text-red-400">Could not load sessions: {error}</p>}
        {sessions === null && error === null && <p className="text-sm text-muted-foreground">Loading sessions</p>}
        {sessions !== null && sessions.length === 0 && (
          <p className="text-sm text-muted-foreground">No sessions recorded yet.</p>
        )}
        {sessions !== null && sessions.length > 0 && <SessionTable sessions={sessions} />}
      </Card>
    </main>
  );
}

function SessionTable({ sessions }: { sessions: SessionView[] }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="text-xs uppercase tracking-wider text-muted-foreground">
          <tr>
            <th className="py-1 pr-3 font-medium">Session</th>
            <th className="py-1 pr-3 font-medium">Setup</th>
            <th className="py-1 pr-3 text-right font-medium">Runs</th>
            <th className="py-1 pr-3 font-medium">Headline</th>
            <th className="py-1 pr-3 font-medium">States</th>
          </tr>
        </thead>
        <tbody>
          {sessions.map((v) => {
            const s = summarizeSession(v);
            return (
              <tr key={v.id} className="border-t align-top">
                <td className="py-1 pr-3 font-mono">
                  <Link to="/sessions/$id" params={{ id: v.id }} className="underline underline-offset-2">
                    {v.id}
                  </Link>
                  {v.session !== null && <p className="text-xs text-muted-foreground">{v.session.name}</p>}
                </td>
                <td className="py-1 pr-3">
                  {sessionLabel(v)}
                  {s.error !== null && <p className="text-xs text-red-400">{s.error}</p>}
                </td>
                <td className="py-1 pr-3 text-right font-mono">{s.runCount}</td>
                <td className="py-1 pr-3 font-mono">
                  {s.headline === null ? (
                    "n/a"
                  ) : (
                    <>
                      {formatNumber(s.headline.value, s.type === "probe" ? 3 : 1)} {s.headline.unit}
                      <p className="text-xs text-muted-foreground">{s.headline.note ?? s.headline.label}</p>
                    </>
                  )}
                </td>
                <td className="py-1 pr-3">
                  {s.counts.length === 0 ? (
                    <span className="text-muted-foreground">no runs</span>
                  ) : (
                    s.counts.map((c) => (
                      <span key={c.state} className="mr-3 whitespace-nowrap">
                        {c.count} {stateLabel(c.state)}
                      </span>
                    ))
                  )}
                  {s.verificationFailures > 0 && (
                    <p className="text-xs text-amber-400">{s.verificationFailures} with failed verification</p>
                  )}
                  {s.reasons.map((r) => (
                    <p key={r} className="text-xs text-muted-foreground">
                      {r}
                    </p>
                  ))}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
