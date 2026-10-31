import type { RunEntry, SessionView } from "./api";
import { headlineMetric, runTypeOfKind, type Headline, type RunType } from "./tune";

const STATE_ORDER = ["done", "failed", "not_comparable", "skipped"];

export interface SessionSummary {
  id: string;
  error: string | null;
  type: RunType | null;
  runCount: number;
  counts: { state: string; count: number }[];
  reasons: string[];
  headline: Headline | null;
  verificationFailures: number;
}

/** Newest first. Ids start with their creation time, so id order is time order and total, even for sessions whose file is unreadable. */
export function sortSessions(views: readonly SessionView[]): SessionView[] {
  return [...views].sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
}

function newestDone(runs: readonly RunEntry[]): RunEntry | undefined {
  return runs
    .filter((r) => r.state === "done" && r.run !== null)
    .sort((a, b) => (b.run?.created_unix_s ?? 0) - (a.run?.created_unix_s ?? 0) || b.id.localeCompare(a.id))[0];
}

export function summarizeSession(v: SessionView): SessionSummary {
  const type: RunType | null = v.session === null ? null : runTypeOfKind(v.session.kind);
  const tally = new Map<string, number>();
  const reasons = new Set<string>();
  let verificationFailures = 0;
  for (const r of v.runs) {
    tally.set(r.state, (tally.get(r.state) ?? 0) + 1);
    if ("reason" in r) reasons.add(r.reason);
    if (r.run?.verification.some((c) => !c.passed)) verificationFailures += 1;
  }
  const rank = (s: string) => {
    const i = STATE_ORDER.indexOf(s);
    return i < 0 ? STATE_ORDER.length : i;
  };
  const counts = [...tally]
    .map(([state, count]) => ({ state, count }))
    .sort((a, b) => rank(a.state) - rank(b.state) || a.state.localeCompare(b.state));
  const best = newestDone(v.runs);
  return {
    id: v.id,
    error: v.error,
    type,
    runCount: v.runs.length,
    counts,
    reasons: [...reasons].sort((a, b) => a.localeCompare(b)),
    headline: best?.run == null || type === null ? null : headlineMetric(best.run, type),
    verificationFailures,
  };
}

function point(p: [number, number]): string {
  return `(${p[0]}, ${p[1]})`;
}

/** One line naming the stepper, run kind and bed pattern a session was set up with. */
export function sessionLabel(v: SessionView): string {
  const s = v.session;
  if (s === null) return "unreadable session";
  return `${s.stepper}, ${runTypeOfKind(s.kind)}, ${point(s.pattern.start)} to ${point(s.pattern.end)}`;
}
