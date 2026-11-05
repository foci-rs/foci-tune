import { createRoute, Link, useNavigate } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { getSessions, getTunes, type SessionView, type TuneSummary } from "@/api";
import { Pager } from "@/components/Pager";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Card } from "@/components/ui";
import { useFeed } from "@/feed";
import { stateText } from "@/guided";
import { formatStarted, type Page, paginate, sessionRow, sortSessions } from "@/sessions";
import { Route as rootRoute } from "./__root";

export const Route = createRoute({
  getParentRoute: () => rootRoute,
  path: "/sessions",
  validateSearch: (search: Record<string, unknown>): { tab?: "guided"; page?: number; tunes?: number } => {
    const number = (v: unknown) => (Number.isInteger(Number(v)) && Number(v) > 1 ? Number(v) : undefined);
    return {
      tab: search.tab === "guided" ? "guided" : undefined,
      page: number(search.page),
      tunes: number(search.tunes),
    };
  },
  component: SessionsPage,
});

function SessionsPage() {
  const { listen } = useFeed();
  const { tab = "manual", page: requested = 1, tunes: tunesRequested = 1 } = Route.useSearch();
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
  const tunesPage = paginate(tunes, tunesRequested);
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

  const manual = sessions?.filter((v) => v.tune === null) ?? null;

  return (
    <main className="mx-auto max-w-6xl space-y-4 px-4 py-6">
      <Tabs
        value={tab}
        onValueChange={(next) =>
          navigate({ to: "/sessions", search: (prev) => ({ ...prev, tab: next === "guided" ? "guided" : undefined }) })
        }
      >
        <TabsList variant="line" aria-label="Kind of tuning">
          <TabsTrigger value="manual">Manual</TabsTrigger>
          <TabsTrigger value="guided">Guided</TabsTrigger>
        </TabsList>
      </Tabs>
      {tab === "manual" ? (
        <Card label="Manual sessions" titleless>
          {error !== null && <p className="text-sm text-red-400">Could not load sessions: {error}</p>}
          {manual === null && error === null && <p className="text-sm text-muted-foreground">Loading sessions</p>}
          {manual !== null && manual.length === 0 && (
            <p className="text-sm text-muted-foreground">No manual sessions recorded yet.</p>
          )}
          {manual !== null && manual.length > 0 && <SessionList page={paginate(manual, requested)} />}
        </Card>
      ) : (
        <Card label="Guided tunes" titleless>
          {tunes.length === 0 && <p className="text-sm text-muted-foreground">No finished tunes yet.</p>}
          {tunes.length > 0 && (
            <>
              <TuneTable tunes={tunesPage.items} />
              <Pager page={tunesPage} param="tunes" label="Tunes pages" />
            </>
          )}
        </Card>
      )}
    </main>
  );
}

function SessionList({ page }: { page: Page<SessionView> }) {
  return (
    <>
      <SessionTable sessions={page.items} />
      <Pager page={page} param="page" label="Sessions pages" />
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

function TuneTable({ tunes }: { tunes: TuneSummary[] }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="text-xs text-muted-foreground">
          <tr>
            <th className="py-1 pr-3 font-medium">Started</th>
            <th className="py-1 pr-3 font-medium">Stepper</th>
            <th className="py-1 pr-3 font-medium">State</th>
          </tr>
        </thead>
        <tbody>
          {tunes.map((t) => (
            <tr key={t.id} className="border-t">
              <td className="py-1 pr-3 font-mono">
                <Link to="/tune/guided" search={{ tune: t.id }} title={t.id} className="underline underline-offset-2">
                  {formatStarted(t.created)}
                </Link>
              </td>
              <td className="py-1 pr-3 font-mono">{t.stepper}</td>
              <td className="py-1 pr-3">{stateText(t.state)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
