import { createRoute, Link, useNavigate } from "@tanstack/react-router";
import { ChevronLeftIcon, ChevronRightIcon } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { getSessions, getTunes, type SessionView, type TuneSummary } from "@/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui";
import { useFeed } from "@/feed";
import { stateText } from "@/guided";
import { formatStarted, type Page, paginate, sessionRow, sortSessions } from "@/sessions";
import { Route as rootRoute } from "./__root";

export const Route = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sessions",
  validateSearch: (search: Record<string, unknown>): { page?: number } => {
    const page = Number(search.page);
    return Number.isInteger(page) && page > 1 ? { page } : {};
  },
  component: SessionsPage,
});

function SessionsPage() {
  const { listen } = useFeed();
  const { page: requested = 1 } = Route.useSearch();
  const navigate = useNavigate();
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
        {sessions !== null && sessions.length > 0 && (
          <SessionList
            page={paginate(sessions, requested)}
            onPage={(page) => navigate({ to: "/sessions", search: page > 1 ? { page } : {} })}
          />
        )}
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

function SessionList({ page, onPage }: { page: Page<SessionView>; onPage: (page: number) => void }) {
  return (
    <>
      <SessionTable sessions={page.items} />
      {page.pages > 1 && (
        <div className="mt-3 flex items-center justify-end gap-2 text-sm text-muted-foreground">
          <span>
            {page.from}–{page.to} of {page.total}
          </span>
          <Button
            variant="outline"
            size="icon"
            aria-label="Previous page"
            disabled={page.page <= 1}
            onClick={() => onPage(page.page - 1)}
          >
            <ChevronLeftIcon />
          </Button>
          <Button
            variant="outline"
            size="icon"
            aria-label="Next page"
            disabled={page.page >= page.pages}
            onClick={() => onPage(page.page + 1)}
          >
            <ChevronRightIcon />
          </Button>
        </div>
      )}
    </>
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
