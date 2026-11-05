import { createRoute, Link } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { getSessions, getTunes, type SessionView, type TuneSummary } from "@/api";
import { Card } from "@/components/ui";
import { useFeed } from "@/feed";
import { stateText } from "@/guided";
import { formatStarted, sessionRow, sortSessions } from "@/sessions";
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
  const [tunes, setTunes] = useState<TuneSummary[]>([]);
  const loadTunes = useCallback(() => {
    getTunes().then(
      (list) =>
        setTunes(
          list.filter((t) => t.state === "Done" || t.state === "Abandoned").sort((a, b) => b.created - a.created),
        ),
      () => undefined,
    );
  }, []);
  useEffect(loadTunes, [loadTunes]);
  useEffect(
    () =>
      listen((m) => {
        if (m.type === "run_state" || m.type === "run_result" || m.type === "resync") refresh();
        if (m.type === "tune" || m.type === "resync") loadTunes();
      }),
    [listen, refresh, loadTunes],
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
      {tunes.length > 0 && (
        <Card label="Tunes">
          <ul className="divide-y text-sm">
            {tunes.map((t) => (
              <li key={t.id}>
                <Link
                  to="/tune/guided"
                  search={{ tune: t.id }}
                  className="flex items-baseline gap-4 py-1.5 hover:underline"
                >
                  <span className="font-mono">{t.stepper}</span>
                  <span className="text-muted-foreground">{stateText(t.state)}</span>
                  <span className="ml-auto text-xs text-muted-foreground">{formatStarted(t.created)}</span>
                </Link>
              </li>
            ))}
          </ul>
        </Card>
      )}
    </main>
  );
}

function SessionTable({ sessions }: { sessions: SessionView[] }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="text-xs text-muted-foreground">
          <tr>
            <th className="py-1 pr-3 font-medium">Started</th>
            <th className="py-1 pr-3 font-medium">Stepper</th>
            <th className="py-1 pr-3 font-medium">Test</th>
            <th className="py-1 pr-3 text-right font-medium">Runs</th>
          </tr>
        </thead>
        <tbody>
          {sessions.map((v) => {
            const row = sessionRow(v);
            return (
              <tr key={row.id} className="border-t">
                <td className="py-1 pr-3 font-mono">
                  <Link
                    to="/sessions/$id"
                    params={{ id: row.id }}
                    title={row.id}
                    className="underline underline-offset-2"
                  >
                    {row.started}
                  </Link>
                </td>
                <td className="py-1 pr-3 font-mono">{row.stepper}</td>
                <td className="py-1 pr-3">{row.test}</td>
                <td className="py-1 pr-3 text-right font-mono">{row.runs}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
