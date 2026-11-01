import { describe, expect, test } from "vitest";
import type { Tune, TuneState, TuneStep } from "./api";
import {
  availableActions,
  canChoose,
  defaultProbeSpeed,
  expectedDurationSeconds,
  rungView,
  stepSummary,
} from "./guided";
import { expectedProbeEdges, PROBE_MIN_EDGES } from "./tune";

const gains = { velocity_p: 1000, velocity_i: 50, position_p: 800, position_i: 0 };
const filters = { velocity_hz: 500, torque_hz: 1000, position_hz: 500, flux_hz: 1000 };

function rung(index: number, over: Partial<Tune["steps"][0]["rungs"][0]> = {}) {
  return {
    shown: 1000 * 2 ** (index / 3),
    raw: 1000 + index * 100,
    run_ids: [],
    started_unix_ms: null,
    ended_unix_ms: null,
    skipped: null,
    repeat_values: [],
    run_times: [],
    ...over,
  };
}

function step(over: Partial<TuneStep> = {}): TuneStep {
  return {
    axis: "VelocityP",
    seed_raw: 1000,
    repeats: 1,
    lower_k: 0,
    upper_k: 3,
    pwm_hz: 50000,
    base: [gains, filters],
    sessions: { probe: "s1", tracking: "s2", standstill: "s3" },
    rungs: [rung(0), rung(1), rung(2), rung(3)],
    notes: [],
    chosen: null,
    chosen_shown: null,
    starts: 1,
    started_unix_ms: null,
    ended_unix_ms: null,
    failed: null,
    assessment: null,
    recommendation: null,
    drift_rung: null,
    drift: [],
    ...over,
  };
}

function tune(state: TuneState, steps: TuneStep[] = [step()]): Tune {
  return {
    format_version: 1,
    id: "t1",
    setup: {
      stepper: "stepper_x",
      speeds_mm_s: [50, 100],
      pattern: { start: [100, 100], end: [200, 200] },
      probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 },
      tracking_strokes: 2,
      standstill: { rests: 3, dwell_s: 2 },
      gates: { damping_floor: 0.05, marginal: 0.08, standstill_factor: 2 },
      include_marginal: false,
    },
    snapshot: [gains, filters],
    steps,
    state,
    created_unix_s: 0,
    notes: [],
  };
}

const none = { inadmissible: null, marginal: false, unscored: null, score: null };
const medians = (p99: (number | null)[]) => ({ zeta: 0.1, p99_by_speed: p99, worst_torque_sd: 1 });

function assessed(overrides: Partial<TuneStep> = {}): TuneStep {
  return step({
    assessment: {
      verdicts: [
        { ...none, unscored: "TrackingMissing" },
        { ...none, inadmissible: "DampingBelowFloor" },
        { ...none, marginal: true, score: 1.4 },
        { ...none, score: 1.1 },
      ],
      medians: [
        medians([null, null]),
        medians([5, 9]),
        medians([4, 8]),
        medians([3, 6]),
      ],
      halt: null,
    },
    ...overrides,
  });
}

describe("rungView", () => {
  test("states why a rung is unscored and why another is inadmissible", () => {
    const rows = rungView(tune("AwaitingChoice", [assessed()]), 0);
    expect(rows[0].verdict).toMatch(/unscored/i);
    expect(rows[0].verdict).toMatch(/tracking missing/i);
    expect(rows[1].verdict).toMatch(/inadmissible/i);
    expect(rows[1].verdict).toMatch(/damping below (the )?floor/i);
  });

  test("names the standstill reasons", () => {
    const s = assessed();
    s.assessment!.verdicts[0] = { ...none, unscored: "StandstillMissing" };
    s.assessment!.verdicts[1] = { ...none, inadmissible: "StandstillHigh" };
    const rows = rungView(tune("AwaitingChoice", [s]), 0);
    expect(rows[0].verdict).toMatch(/standstill missing/i);
    expect(rows[1].verdict).toMatch(/standstill high/i);
  });

  test("flags marginal rungs and carries score, gain and per-speed p99", () => {
    const rows = rungView(tune("AwaitingChoice", [assessed()]), 0);
    expect(rows[2].marginal).toBe(true);
    expect(rows[2].score).toBe(1.4);
    expect(rows[3].marginal).toBe(false);
    expect(rows[3].p99BySpeed).toEqual([3, 6]);
    expect(rows[3].raw).toBe(1300);
    expect(rows[3].shown).toBeCloseTo(2000);
  });

  test("marks a measured recommendation on its rung", () => {
    const s = assessed({ recommendation: { choice: { Measured: { rung: 3 } }, extend: "Above" } });
    const rows = rungView(tune("AwaitingChoice", [s]), 0);
    expect(rows.map((r) => r.recommended)).toEqual([null, null, null, "measured"]);
  });

  test("marks an interpolated recommendation on both bracketing rungs", () => {
    const s = assessed({
      recommendation: { choice: { Interpolated: { shown: 1700, between: [2, 3] } }, extend: null },
    });
    const rows = rungView(tune("AwaitingChoice", [s]), 0);
    expect(rows.map((r) => r.recommended)).toEqual([null, null, "interpolated", "interpolated"]);
    expect(stepSummary(tune("AwaitingChoice", [s]), 0).recommendation).toEqual({
      kind: "interpolated",
      shown: 1700,
      between: [2, 3],
    });
  });

  test("shows a skipped rung with its reason", () => {
    const s = step({ rungs: [rung(0, { skipped: "run failed earlier" }), rung(1)] });
    const rows = rungView(tune("Running", [s]), 0);
    expect(rows[0].skipped).toBe("run failed earlier");
    expect(rows[0].verdict).toMatch(/skipped.*run failed earlier/i);
  });

  test("shows repeat progress before the verdict exists", () => {
    const repeat = { zeta: { Assessed: 0.1 }, p99_by_speed: [3, 4], worst_torque_sd: 1 };
    const s = step({ repeats: 2, rungs: [rung(0, { repeat_values: [repeat, repeat] }), rung(1, { repeat_values: [repeat] }), rung(2)] });
    const rows = rungView(tune("Running", [s]), 0);
    expect(rows.map((r) => r.verdict)).toEqual(["Measured, awaiting verdict", "1 of 2 repeats", "Pending"]);
  });

  test("explains a rung left unassessed after a halt", () => {
    const s = step({
      assessment: {
        verdicts: [{ ...none, score: 1 }, { ...none, inadmissible: "DampingBelowFloor" }, { ...none }, { ...none }],
        medians: [medians([1]), medians([1]), medians([]), medians([])],
        halt: [1, { BelowFloor: { median: 0.03 } }],
      },
    });
    const rows = rungView(tune("AwaitingChoice", [s]), 0);
    expect(rows[2].verdict).toMatch(/not assessed/i);
    expect(rows[2].verdict).toMatch(/stopped/i);
  });

  test("lists omitted and merged rungs as notes", () => {
    const s = step({ notes: [{ OutOfRange: 99999 }, { RoundedToZero: 0.1 }, { Merged: { shown: 1001, into: 1000 } }] });
    const notes = stepSummary(tune("Running", [s]), 0).notes;
    expect(notes).toHaveLength(3);
    expect(notes[0]).toMatch(/99999/);
    expect(notes[2]).toMatch(/1001.*1000/);
  });
});

describe("canChoose", () => {
  test.each<[TuneState, boolean]>([
    ["Ready", false],
    ["Announced", false],
    ["Running", false],
    ["AwaitingChoice", true],
    ["Done", false],
    ["Abandoned", false],
    ["RestorePending", false],
    [{ RestoreFailed: "x" }, false],
  ])("%j -> %s", (state, expected) => {
    expect(canChoose(tune(state))).toBe(expected);
  });
});

describe("availableActions", () => {
  test("offers Start only when a step is announced", () => {
    const states: TuneState[] = ["Ready", "Announced", "Running", "AwaitingChoice", "Done", "Abandoned"];
    const withStart = states.filter((s) => availableActions(tune(s), null).start);
    expect(withStart).toEqual(["Announced"]);
  });

  test("offers choosing and extension only while awaiting a choice", () => {
    const awaiting = availableActions(tune("AwaitingChoice"), null);
    expect(awaiting.choose).toBe(true);
    const running = availableActions(tune("Running"), null);
    expect(running.choose).toBe(false);
    expect(running.extension).toBe(false);
  });

  test("a restore-pending tune offers only restore and acknowledge", () => {
    const a = availableActions(tune("RestorePending"), null);
    expect(a).toEqual({
      prepare: false,
      start: false,
      choose: false,
      extension: false,
      abandon: false,
      restore: true,
      acknowledge: true,
    });
  });

  test("a restore-failed tune or a lock reason also limits to restore and acknowledge", () => {
    const failed = availableActions(tune({ RestoreFailed: "no link" }), null);
    expect([failed.restore, failed.acknowledge, failed.abandon, failed.start, failed.choose]).toEqual([true, true, false, false, false]);
    const locked = availableActions(tune("AwaitingChoice"), "stepper locked");
    expect([locked.restore, locked.acknowledge, locked.abandon, locked.choose]).toEqual([true, true, false, false]);
  });

  test("abandon is offered in every live state and nothing is offered once over", () => {
    for (const s of ["Ready", "Announced", "Running", "AwaitingChoice"] as TuneState[]) {
      expect(availableActions(tune(s), null).abandon).toBe(true);
    }
    for (const s of ["Done", "Abandoned"] as TuneState[]) {
      expect(Object.values(availableActions(tune(s), null)).some(Boolean)).toBe(false);
    }
  });

  test("a new step can be prepared only from Ready", () => {
    expect(availableActions(tune("Ready"), null).prepare).toBe(true);
    expect(availableActions(tune("Announced"), null).prepare).toBe(false);
  });
});

describe("expectedDurationSeconds", () => {
  const timed = (ms: number[]) =>
    tune("Done", [
      step({
        rungs: [
          rung(0, {
            run_times: ms.map((d, i) => ({ id: `r${i}`, started_unix_ms: 1000 * i, ended_unix_ms: 1000 * i + d })),
          }),
        ],
      }),
    ]);

  test("scales the mean recorded run time by the run count", () => {
    expect(expectedDurationSeconds([timed([10_000, 20_000])], 4)).toBe(60);
  });

  test("is null without any recorded run", () => {
    expect(expectedDurationSeconds([], 4)).toBeNull();
    expect(expectedDurationSeconds([tune("Done")], 4)).toBeNull();
  });
});

describe("defaultProbeSpeed", () => {
  const context = {
    pattern: { start: [100.5, 104] as [number, number], end: [200.5, 204] as [number, number] },
    kinematics: "corexy",
    segmentsBetweenToggles: 6,
    strokes: 20,
    maxVelocity: 500,
  };
  const edges = (speedMmS: number, over: Partial<typeof context> = {}) =>
    expectedProbeEdges({ ...context, ...over, speedMmS });

  test("picks the fastest speed that reaches the edge minimum", () => {
    expect(edges(400)).toBeGreaterThanOrEqual(PROBE_MIN_EDGES);
    expect(defaultProbeSpeed([195, 400, 100], context)).toEqual({ speed: 400, warning: null });
  });

  test("skips a faster speed that falls short of the minimum", () => {
    const ctx = { ...context, strokes: 12 };
    expect(expectedProbeEdges({ ...ctx, speedMmS: 400 })).toBeLessThan(PROBE_MIN_EDGES);
    expect(expectedProbeEdges({ ...ctx, speedMmS: 195 })).toBeGreaterThanOrEqual(PROBE_MIN_EDGES);
    expect(defaultProbeSpeed([400, 195], ctx).speed).toBe(195);
    expect(defaultProbeSpeed([400, 195], ctx).warning).toBeNull();
  });

  test("falls back to the fastest valid speed with a warning when none reaches the minimum", () => {
    const ctx = { ...context, strokes: 8 };
    const result = defaultProbeSpeed([400], ctx);
    expect(result.speed).toBe(400);
    expect(result.warning).toMatch(/edges/);
  });

  test("ignores speeds whose toggles are too close together", () => {
    const ctx = { ...context, segmentsBetweenToggles: 3, strokes: 40 };
    expect(defaultProbeSpeed([600, 195], ctx).speed).toBe(195);
  });

  test("warns and keeps the fastest speed when no speed has valid toggle spacing", () => {
    const ctx = { ...context, segmentsBetweenToggles: 1 };
    const result = defaultProbeSpeed([300, 600], ctx);
    expect(result.speed).toBe(600);
    expect(result.warning).toMatch(/toggle/i);
  });

  test("judges a speed above max_velocity at the clamped speed, as the probe runs it", () => {
    const ctx = { ...context, maxVelocity: 300, strokes: 40 };
    const clamped = expectedProbeEdges({ ...ctx, speedMmS: 600 });
    expect(clamped).toBe(expectedProbeEdges({ ...ctx, speedMmS: 300 * Math.SQRT2 }));
    expect(clamped).toBeGreaterThanOrEqual(PROBE_MIN_EDGES);
    expect(defaultProbeSpeed([600, 195], ctx)).toEqual({ speed: 600, warning: null });
  });

  test("warns when there is no speed to choose from", () => {
    expect(defaultProbeSpeed([], context).warning).not.toBeNull();
  });
});
