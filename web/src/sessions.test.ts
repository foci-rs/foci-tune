import { describe, expect, test } from "vitest";
import type { RunEntry, RunKind, SessionView } from "./api";
import { runTypeOfKind } from "./tune";
import { formatStarted, pageLinks, paginate, sessionLabel, sessionRow, sortSessions } from "./sessions";

const probeKind: RunKind = {
  Probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 },
};

function view(id: string, created: number | null, runs: RunEntry[] = [], kind: RunKind = probeKind): SessionView {
  return {
    id,
    tune: null,
    error: created === null ? "session.json: missing" : null,
    session:
      created === null
        ? null
        : ({
            name: id,
            created_unix_s: created,
            stepper: "stepper_x",
            kind,
            pattern: { start: [100, 50], end: [200, 50] },
          } as unknown as SessionView["session"]),
    runs,
  };
}

describe("sortSessions", () => {
  test("newest first by id, which starts with the creation time", () => {
    const sorted = sortSessions([view("20261029-a", 10), view("20261031-b", 30), view("20261030-c", 20)]);
    expect(sorted.map((s) => s.id)).toEqual(["20261031-b", "20261030-c", "20261029-a"]);
  });

  test("equal creation times keep a stable id order, and the input is untouched", () => {
    const input = [view("2026-1", 5), view("2026-3", 5), view("2026-2", 5)];
    expect(sortSessions(input).map((s) => s.id)).toEqual(["2026-3", "2026-2", "2026-1"]);
    expect(input.map((s) => s.id)).toEqual(["2026-1", "2026-3", "2026-2"]);
  });

  test("an unreadable session sorts among the others, never dropped", () => {
    const sorted = sortSessions([view("20261030-a", 100), view("20261031-b", null)]);
    expect(sorted.map((s) => s.id)).toEqual(["20261031-b", "20261030-a"]);
  });
});

describe("labels", () => {
  test("session label names stepper, kind and pattern", () => {
    expect(sessionLabel(view("s", 1))).toBe("stepper_x, probe, (100, 50) to (200, 50)");
    expect(sessionLabel(view("s", null))).toBe("unreadable session");
  });

  test("runTypeOfKind maps each kind", () => {
    expect(runTypeOfKind({ Tracking: { speeds_mm_s: [1], strokes: 1 } })).toBe("tracking");
    expect(runTypeOfKind(probeKind)).toBe("probe");
    expect(runTypeOfKind({ Standstill: { rests: 1, dwell_s: 1 } })).toBe("standstill");
  });
});

describe("the session list row", () => {
  const local = (y: number, mo: number, d: number, h: number, mi: number) =>
    new Date(y, mo - 1, d, h, mi, 30).getTime() / 1000;

  test("the start is the local date and a 24-hour time to the minute", () => {
    expect(formatStarted(local(2026, 11, 4, 22, 7))).toBe("2026-11-04 22:07");
    expect(formatStarted(local(2026, 1, 9, 0, 5))).toBe("2026-01-09 00:05");
    expect(formatStarted(local(2026, 11, 4, 13, 30))).not.toMatch(/[ap]m/i);
  });

  test("a row names the start, the stepper, the test and the number of runs", () => {
    const runs = [1, 2, 3].map((n) => ({ id: `r${n}`, state: "failed", reason: "x", run: null }) as RunEntry);
    const row = sessionRow(view("20261104T222217Z-0r0j", local(2026, 11, 4, 22, 22), runs));
    expect(row).toEqual({
      id: "20261104T222217Z-0r0j",
      started: "2026-11-04 22:22",
      stepper: "stepper_x",
      test: "Probe",
      runs: 3,
    });
  });

  test("each kind of run has its own test name", () => {
    const tracking: RunKind = { Tracking: { speeds_mm_s: [100], strokes: 8 } };
    const standstill: RunKind = { Standstill: { rests: 8, dwell_s: 3 } };
    expect(sessionRow(view("a", 1, [], tracking)).test).toBe("Tracking");
    expect(sessionRow(view("a", 1, [], standstill)).test).toBe("Standstill");
  });

  test("a session whose file cannot be read still has a row that says so", () => {
    expect(sessionRow(view("20261030T000000Z-bad1", null))).toEqual({
      id: "20261030T000000Z-bad1",
      started: "unknown",
      stepper: "unknown",
      test: "Unreadable",
      runs: 0,
    });
  });
});

describe("paginate", () => {
  const items = Array.from({ length: 101 }, (_, i) => i);

  test("a page holds 25 items and says which ones", () => {
    const first = paginate(items, 1);
    expect(first.items).toEqual(items.slice(0, 25));
    expect(first).toMatchObject({ page: 1, pages: 5, from: 1, to: 25, total: 101 });
    const last = paginate(items, 5);
    expect(last.items).toEqual([100]);
    expect(last).toMatchObject({ page: 5, from: 101, to: 101 });
  });

  test("a page past either end is clamped to the nearest page", () => {
    expect(paginate(items, 99).page).toBe(5);
    expect(paginate(items, 0).page).toBe(1);
  });

  test("an empty list is one empty page", () => {
    expect(paginate([], 3)).toEqual({ items: [], page: 1, pages: 1, from: 0, to: 0, total: 0 });
  });
});

describe("pageLinks", () => {
  test("a few pages are all listed", () => {
    expect(pageLinks(1, 1)).toEqual([1]);
    expect(pageLinks(3, 5)).toEqual([1, 2, 3, 4, 5]);
  });

  test("a long list keeps the first and last page and the neighbours of the current one", () => {
    expect(pageLinks(1, 20)).toEqual([1, 2, "gap", 20]);
    expect(pageLinks(10, 20)).toEqual([1, "gap", 9, 10, 11, "gap", 20]);
    expect(pageLinks(20, 20)).toEqual([1, "gap", 19, 20]);
  });

  test("a gap of one page lists that page instead of a gap", () => {
    expect(pageLinks(4, 20)).toEqual([1, 2, 3, 4, 5, "gap", 20]);
    expect(pageLinks(17, 20)).toEqual([1, "gap", 16, 17, 18, 19, 20]);
  });
});
