import { describe, expect, test } from "vitest";
import type { Run, RunEntry, RunKind, SessionView } from "./api";
import { runTypeOfKind } from "./tune";
import { sessionLabel, sortSessions, summarizeSession } from "./sessions";

const probeKind: RunKind = {
  Probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 },
};

function view(id: string, created: number | null, runs: RunEntry[] = [], kind: RunKind = probeKind): SessionView {
  return {
    id,
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

function probeRun(id: string, created: number, zeta: number | null): Run {
  return {
    id,
    created_unix_s: created,
    declared_channel: 0,
    verification: [],
    metrics: {
      channels: [
        {
          channel: 0,
          declared: true,
          tracking: [],
          standstill: null,
          probe: {
            velocity: zeta === null ? { status: "failed", reason: "no fit" } : { status: "fit", zeta },
            edges: 3,
          },
        },
      ],
    },
    state: "done",
  } as unknown as Run;
}

const done = (id: string, created: number, zeta: number | null): RunEntry =>
  ({ id, state: "done", run: probeRun(id, created, zeta) }) as RunEntry;

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

describe("summarizeSession", () => {
  test("counts every state and keeps each reason", () => {
    const runs = [
      done("r1", 1, 0.1),
      { id: "r2", state: "skipped", reason: "floor reached", run: null },
      { id: "r3", state: "failed", reason: "usb lost", run: null },
      { id: "r4", state: "not_comparable", reason: "gains drifted", run: null },
      { id: "r5", state: "failed", reason: "usb lost", run: null },
    ] as RunEntry[];
    const s = summarizeSession(view("s", 1, runs));
    expect(s.runCount).toBe(5);
    expect(s.counts).toEqual([
      { state: "done", count: 1 },
      { state: "failed", count: 2 },
      { state: "not_comparable", count: 1 },
      { state: "skipped", count: 1 },
    ]);
    expect(s.reasons).toEqual(["floor reached", "usb lost", "gains drifted"].sort((a, b) => a.localeCompare(b)));
  });

  test("headline is the newest done run's metric for the session's run kind", () => {
    const s = summarizeSession(view("s", 1, [done("r1", 1, 0.3), done("r2", 2, 0.12)]));
    expect(s.type).toBe("probe");
    expect(s.headline).toMatchObject({ label: "velocity zeta", value: 0.12 });
  });

  test("no done run with a record means no headline, and an unreadable session keeps its error", () => {
    expect(summarizeSession(view("s", 1, [])).headline).toBeNull();
    const broken = summarizeSession(
      view("x", null, [{ id: "r", state: "failed", reason: "bad", run: null } as RunEntry]),
    );
    expect(broken.type).toBeNull();
    expect(broken.error).toBe("session.json: missing");
    expect(broken.reasons).toEqual(["bad"]);
  });

  test("a verification failure is counted", () => {
    const bad = done("r1", 1, 0.2);
    (bad.run as Run).verification = [{ check: "channel_moved", passed: false, detail: "no motion" }];
    expect(summarizeSession(view("s", 1, [bad])).verificationFailures).toBe(1);
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
