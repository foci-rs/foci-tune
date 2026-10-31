import { createRoute, Link } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { getSession, type SessionView } from "@/api";
import { Card } from "@/components/ui";
import { useFeed } from "@/feed";
import { sessionLabel } from "@/sessions";
import { SessionResults } from "./tune";
import { Route as rootRoute } from "./__root";

export const Route = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sessions/$id",
  component: SessionPage,
});

function SessionPage() {
  const { id } = Route.useParams();
  const { listen } = useFeed();
  const [view, setView] = useState<SessionView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);

  const refresh = useCallback(() => {
    const mine = ++generation.current;
    getSession(id).then(
      (v) => {
        if (mine !== generation.current) return;
        setView(v);
        setError(null);
      },
      (e: unknown) => {
        if (mine === generation.current) setError(e instanceof Error ? e.message : "request failed");
      },
    );
  }, [id]);

  useEffect(() => {
    setView(null);
    refresh();
  }, [refresh]);
  useEffect(
    () =>
      listen((m) => {
        if (((m.type === "run_state" || m.type === "run_result") && m.session === id) || m.type === "resync") {
          refresh();
        }
      }),
    [listen, id, refresh],
  );

  return (
    <main className="mx-auto max-w-6xl space-y-4 px-4 py-6">
      <p className="text-sm">
        <Link to="/sessions" className="underline underline-offset-2">
          All sessions
        </Link>
      </p>
      {error !== null && <p className="text-sm text-red-400">Could not load session {id}: {error}</p>}
      {view === null && error === null && <p className="text-sm text-muted-foreground">Loading session</p>}
      {view !== null && (
        <>
          <Card label={`Session ${view.id}`}>
            <p className="font-mono text-sm">{sessionLabel(view)}</p>
            {view.session !== null && (
              <p className="mt-1 text-xs text-muted-foreground">
                {view.session.name}, recorded {new Date(view.session.created_unix_s * 1000).toLocaleString()}. Read only.
              </p>
            )}
            {view.error !== null && <p className="mt-2 text-sm text-red-400">{view.error}</p>}
          </Card>
          {view.session !== null && <SessionResults view={view} />}
          {view.session === null && view.runs.length > 0 && (
            <Card label="Runs">
              <ul className="space-y-1 font-mono text-sm">
                {view.runs.map((r) => (
                  <li key={r.id}>
                    {r.id}: {r.state.replace(/_/g, " ")}
                    {"reason" in r && <span className="text-muted-foreground"> ({r.reason})</span>}
                  </li>
                ))}
              </ul>
            </Card>
          )}
        </>
      )}
    </main>
  );
}
