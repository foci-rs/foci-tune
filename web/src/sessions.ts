import type { SessionView } from "./api";
import { runTypeOfKind, type RunType } from "./tune";

/** Newest first. Ids start with their creation time, so id order is time order and total, even for sessions whose file is unreadable. */
export function sortSessions(views: readonly SessionView[]): SessionView[] {
  return [...views].sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
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

const two = (n: number) => String(n).padStart(2, "0");

/** Local time as `YYYY-MM-DD HH:MM`, 24-hour. */
export function formatStarted(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}`;
}

export interface SessionRow {
  id: string;
  started: string;
  stepper: string;
  test: string;
  runs: number;
}

const TEST_NAMES: Record<RunType, string> = { probe: "Probe", tracking: "Tracking", standstill: "Standstill" };

/** What one line of the session list shows. */
export function sessionRow(v: SessionView): SessionRow {
  const s = v.session;
  if (s === null) return { id: v.id, started: "unknown", stepper: "unknown", test: "Unreadable", runs: v.runs.length };
  return {
    id: v.id,
    started: formatStarted(s.created_unix_s),
    stepper: s.stepper,
    test: TEST_NAMES[runTypeOfKind(s.kind)],
    runs: v.runs.length,
  };
}

export const PAGE_SIZE = 25;

export interface Page<T> {
  items: T[];
  page: number;
  pages: number;
  from: number;
  to: number;
  total: number;
}

/** One page of `items`, counted from 1. A page past either end becomes the nearest page. */
export function paginate<T>(items: readonly T[], page: number, size: number = PAGE_SIZE): Page<T> {
  const pages = Math.max(1, Math.ceil(items.length / size));
  const at = Number.isFinite(page) ? Math.min(pages, Math.max(1, Math.trunc(page))) : 1;
  const start = (at - 1) * size;
  const shown = items.slice(start, start + size);
  return {
    items: shown,
    page: at,
    pages,
    from: shown.length === 0 ? 0 : start + 1,
    to: start + shown.length,
    total: items.length,
  };
}

/** The page numbers a pager lists: the first, the last and the current page with its neighbours; a hidden run of two or more pages is a `"gap"`. */
export function pageLinks(page: number, pages: number): (number | "gap")[] {
  const wanted = new Set([1, pages, page - 1, page, page + 1]);
  const shown = [...wanted].filter((n) => n >= 1 && n <= pages).sort((a, b) => a - b);
  const out: (number | "gap")[] = [];
  for (const [i, n] of shown.entries()) {
    const before = i === 0 ? null : shown[i - 1];
    if (before !== null && n - before === 2) out.push(n - 1);
    else if (before !== null && n - before > 2) out.push("gap");
    out.push(n);
  }
  return out;
}
