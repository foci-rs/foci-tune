import { describe, expect, test } from "vitest";
import { ApiRequestError } from "./api";
import type { LadderAssessment, RungVerdict, Tune, TuneState, TuneStep } from "./api";
import {
  announcementOf,
  availableActions,
  curveData,
  canChoose,
  currentGuidedStep,
  failureText,
  defaultProbeSpeed,
  expectedDurationSeconds,
  nextAxis,
  recommendedGain,
  driftCheckRunning,
  rungView,
  comparisonText,
  driftLines,
  gatesBody,
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
    waived: null,
    superseded: [],
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
    paused: null,
    standstill_gate_off: null,
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

const none: RungVerdict = {
  inadmissible: null,
  marginal: false,
  unscored: null,
  score: null,
  deciding_speed: null,
  gap: null,
  tolerance: null,
};
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
      medians: [medians([null, null]), medians([5, 9]), medians([4, 8]), medians([3, 6])],
      halt: null,
      best: null,
      runner_up: null,
      tie: [],
      spreads: [],
    },
    ...overrides,
  });
}

describe("driftCheckRunning", () => {
  const drifting = (over: Partial<TuneStep> = {}) => step({ drift_rung: rung(1, { started_unix_ms: 1 }), ...over });

  test("is true from the seed re-run's start until its drift is recorded", () => {
    expect(driftCheckRunning(tune("Running", [drifting()]), 0)).toBe(true);
  });

  test("is false before the re-run starts, once drift is recorded and after the step ends", () => {
    expect(driftCheckRunning(tune("Running", [step()]), 0)).toBe(false);
    const recorded = drifting({ drift: [{ metric: "zeta", first: 0.1, rerun: 0.09, verdict: null }] });
    expect(driftCheckRunning(tune("Running", [recorded]), 0)).toBe(false);
    expect(driftCheckRunning(tune("AwaitingChoice", [drifting({ ended_unix_ms: 5 })]), 0)).toBe(false);
  });
});

describe("rungView", () => {
  test("states why a rung is unscored and why another is inadmissible", () => {
    const rows = rungView(tune("AwaitingChoice", [assessed()]), 0);
    expect(rows[0].verdict).toMatch(/not scored/i);
    expect(rows[0].verdict).toMatch(/no tracking result/i);
    expect(rows[1].verdict).toMatch(/rejected/i);
    expect(rows[1].verdict).toMatch(/damping below the stop limit/i);
  });

  test("a running step shows finished rungs assessed and the others as still to run", () => {
    const s = step({
      repeats: 2,
      rungs: [
        rung(0, { repeat_values: [{} as never, {} as never] }),
        rung(1, { repeat_values: [{} as never] }),
        rung(2),
        rung(3),
      ],
      assessment: {
        verdicts: [{ ...none, score: 1 }, { ...none }, { ...none }, { ...none }],
        medians: [medians([3, 6]), medians([null, null]), medians([null, null]), medians([null, null])],
        halt: null,
        best: null,
        runner_up: null,
        tie: [],
        spreads: [],
      },
    });
    const rows = rungView(tune("Running", [s]), 0);
    expect(rows[0].verdict).toBe("Accepted");
    expect(rows[0].score).toBe(1);
    expect(rows[1].verdict).toBe("1 of 2 repeats");
    expect(rows[2].verdict).toBe("Pending");
  });

  test("names the standstill reasons", () => {
    const s = assessed();
    s.assessment!.verdicts[0] = { ...none, unscored: "StandstillMissing" };
    s.assessment!.verdicts[1] = { ...none, inadmissible: "StandstillHigh" };
    const rows = rungView(tune("AwaitingChoice", [s]), 0);
    expect(rows[0].verdict).toMatch(/no standstill result/i);
    expect(rows[1].verdict).toMatch(/too much noise at standstill/i);
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
    const s = step({
      repeats: 2,
      rungs: [rung(0, { repeat_values: [repeat, repeat] }), rung(1, { repeat_values: [repeat] }), rung(2)],
    });
    const rows = rungView(tune("Running", [s]), 0);
    expect(rows.map((r) => r.verdict)).toEqual(["Measured, waiting for the verdict", "1 of 2 repeats", "Pending"]);
  });

  test("explains a rung left unassessed after a halt", () => {
    const s = step({
      assessment: {
        verdicts: [{ ...none, score: 1 }, { ...none, inadmissible: "DampingBelowFloor" }, { ...none }, { ...none }],
        medians: [medians([1]), medians([1]), medians([]), medians([])],
        halt: [1, { BelowFloor: { median: 0.03 } }],
        best: null,
        runner_up: null,
        tie: [],
        spreads: [],
      },
    });
    const rows = rungView(tune("AwaitingChoice", [s]), 0);
    expect(rows[2].verdict).toMatch(/not checked/i);
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

describe("a record whose assessment predates an extension", () => {
  const stale = (extra: "below" | "above") => {
    const base = assessed({ recommendation: { choice: { Measured: { rung: 3 } }, extend: "Above" } });
    const added = rung(extra === "below" ? -1 : 4);
    base.rungs = extra === "below" ? [added, ...base.rungs] : [...base.rungs, added];
    return tune("Running", [base]);
  };

  test.each(["below", "above"] as const)("ignores the verdicts after an extension %s", (side) => {
    const t = stale(side);
    const rows = rungView(t, 0);
    expect(rows).toHaveLength(5);
    expect(rows.every((r) => r.verdict === "Pending")).toBe(true);
    expect(rows.every((r) => r.p99BySpeed.length === 0 && r.score === null && r.recommended === null)).toBe(true);
    expect(curveData(rows, 2)[1].every((v) => v === null)).toBe(true);
    const summary = stepSummary(t, 0);
    expect(summary.halt).toBeNull();
    expect(summary.recommendation).toBeNull();
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

  test("a restore-pending tune offers only restore", () => {
    const a = availableActions(tune("RestorePending"), null);
    expect(a).toEqual({
      prepare: false,
      start: false,
      choose: false,
      decide: false,
      extension: false,
      abandon: false,
      restore: true,
    });
  });

  test("a restore-failed tune or a lock reason also limits to restore", () => {
    const failed = availableActions(tune({ RestoreFailed: "no link" }), null);
    expect([failed.restore, failed.abandon, failed.start, failed.choose]).toEqual([true, false, false, false]);
    const locked = availableActions(tune("AwaitingChoice"), "stepper locked");
    expect([locked.restore, locked.abandon, locked.choose]).toEqual([true, false, false]);
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
    expect(result.warning).toMatch(/step responses/);
    expect(result.warning).not.toMatch(/edges/i);
  });

  test("ignores speeds whose toggles are too close together", () => {
    const ctx = { ...context, segmentsBetweenToggles: 3, strokes: 40 };
    expect(defaultProbeSpeed([600, 195], ctx).speed).toBe(195);
  });

  test("warns and keeps the fastest speed when no speed has valid toggle spacing", () => {
    const ctx = { ...context, segmentsBetweenToggles: 1 };
    const result = defaultProbeSpeed([300, 600], ctx);
    expect(result.speed).toBe(600);
    expect(result.warning).toMatch(/Moves between gain changes/);
    expect(result.warning).not.toMatch(/toggle/i);
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

describe("announcementOf", () => {
  test("states stepper, axis, rung range and the run count including the drift re-run", () => {
    const s = step({ repeats: 2, seed_raw: 1100, rungs: [rung(0), rung(1), rung(2), rung(3)] });
    const a = announcementOf(tune("Announced", [s]));
    expect(a).toMatchObject({ stepper: "stepper_x", axis: "VelocityP", rungs: 4, repeats: 2 });
    expect(a?.firstGain).toBe(s.rungs[0].shown);
    expect(a?.lastGain).toBe(s.rungs[3].shown);
    expect(a?.runCount).toBe(4 * 2 * 4 + 4);
  });

  test("leaves out the drift re-run when the ladder does not hold the seed", () => {
    expect(announcementOf(tune("Announced", [step({ seed_raw: 5 })]))?.runCount).toBe(4 * 1 * 4);
  });

  test("uses the run count the server stored over the client formula", () => {
    const s = step({ repeats: 2, seed_raw: 0, run_count: 28, rungs: [rung(0), rung(1), rung(2)] });
    expect(announcementOf(tune("Announced", [s]))?.runCount).toBe(28);
    expect(announcementOf(tune("Announced", [{ ...s, run_count: null }]))?.runCount).toBe(3 * 2 * 4);
  });

  test("is null before any step exists", () => {
    expect(announcementOf(tune("Ready", []))).toBeNull();
  });
});

describe("nextAxis", () => {
  test("follows velocity P, velocity I corner, position P and then ends", () => {
    const three = [step(), step({ axis: "VelocityICornerHz" }), step({ axis: "PositionP" })];
    expect([0, 1, 2, 3].map((n) => nextAxis(tune("Ready", three.slice(0, n))))).toEqual([
      "VelocityP",
      "VelocityICornerHz",
      "PositionP",
      null,
    ]);
  });
});

describe("curve", () => {
  test("has a p99 column per speed and a gap where a rung has no result", () => {
    const rows = rungView(tune("AwaitingChoice", [assessed()]), 0);
    const [x, fast, slow] = curveData(rows, 2);
    expect(x).toHaveLength(4);
    expect(fast).toEqual([null, 5, 4, 3]);
    expect(slow).toEqual([null, 9, 8, 6]);
  });

  test("marks the interpolated gain itself and a measured rung at its own gain", () => {
    const interpolated = assessed({
      recommendation: { choice: { Interpolated: { shown: 1700, between: [2, 3] } }, extend: null },
    });
    expect(recommendedGain(tune("AwaitingChoice", [interpolated]), 0)).toEqual({ gain: 1700, kind: "interpolated" });
    const measured = assessed({ recommendation: { choice: { Measured: { rung: 3 } }, extend: null } });
    const t = tune("AwaitingChoice", [measured]);
    expect(recommendedGain(t, 0)).toEqual({ gain: t.steps[0].rungs[3].shown, kind: "measured" });
    expect(recommendedGain(tune("AwaitingChoice", [assessed()]), 0)).toBeNull();
  });
});

describe("a paused step", () => {
  test("offers decide and abandon only", () => {
    const actions = availableActions(tune("Paused"), null);
    expect(actions).toMatchObject({ decide: true, abandon: true, choose: false, extension: false, prepare: false });
  });
  test("summarises the pause with its rung and reason", () => {
    const t = tune("Paused");
    t.steps[0].paused = { rung: 1, reason: "DampingUnassessable", unix_ms: 1 };
    t.steps[0].assessment = {
      verdicts: [],
      medians: [],
      halt: null,
      pause: [1, "DampingUnassessable"],
      best: null,
      runner_up: null,
      tie: [],
      spreads: [],
    };
    expect(stepSummary(t, 0).pause).toBe(
      `Paused at gain ${t.steps[0].rungs[1].shown.toFixed(2)}: the damping could not be measured.`,
    );
  });
  test("explains a waived rung and a paused rung", () => {
    const t = tune("Running");
    t.steps[0].rungs[1].waived = { unix_ms: 1, reason: "DampingUnassessable" };
    t.steps[0].assessment = {
      verdicts: [none, { ...none, unscored: "DampingWaived", score: null }, none, none],
      medians: [medians([null]), medians([null]), medians([null]), medians([null])],
      halt: null,
      best: null,
      runner_up: null,
      tie: [],
      spreads: [],
      pause: [2, "StandstillBaselineUnassessable"],
    };
    const rows = rungView(t, 0);
    expect(rows[1].verdict).toContain("Continued anyway: the damping could not be measured");
    expect(rows[2].verdict).toContain("Not checked: the noise at standstill could not be measured");
  });
  test("a halt of DampingUnassessable from an old record still reads as a halt", () => {
    const t = tune("AwaitingChoice");
    t.steps[0].assessment = {
      verdicts: [],
      medians: [],
      halt: [1, "DampingUnassessable"],
      best: null,
      runner_up: null,
      tie: [],
      spreads: [],
    };
    t.steps[0].assessment.verdicts = [none, none, none, none];
    t.steps[0].assessment.medians = [medians([]), medians([]), medians([]), medians([])];
    expect(stepSummary(t, 0).halt).toContain("The sweep stopped at gain");
    expect(stepSummary(t, 0).halt).toContain("the damping could not be measured");
  });
  test("names the standstill gate as off", () => {
    const t = tune("Running");
    t.steps[0].standstill_gate_off = 5;
    expect(rungView(t, 0)[0].verdict).toContain("standstill check off");
  });
});

describe("gatesBody", () => {
  test("posts no factor while the gate is off and the factor while on", () => {
    const form = { damping_floor: 0.02, marginal: 0.025, standstill_gate: false, standstill_factor: 2 };
    expect(gatesBody(form)).toEqual({ damping_floor: 0.02, marginal: 0.025, standstill_factor: null });
    expect(gatesBody({ ...form, standstill_gate: true })).toEqual({
      damping_floor: 0.02,
      marginal: 0.025,
      standstill_factor: 2,
    });
  });
});

test("a setup without a standstill factor labels every rung gate-off once", () => {
  const off = tune("AwaitingChoice", [step({ standstill_gate_off: 123 })]);
  off.setup.gates.standstill_factor = null;
  for (const row of rungView(off, 0)) expect(row.verdict.match(/\(standstill check off\)/g)).toHaveLength(1);

  const setupOnly = tune("AwaitingChoice", [step()]);
  setupOnly.setup.gates.standstill_factor = null;
  for (const row of rungView(setupOnly, 0)) expect(row.verdict.match(/\(standstill check off\)/g)).toHaveLength(1);

  const on = tune("AwaitingChoice", [step({ standstill_gate_off: null })]);
  on.setup.gates.standstill_factor = 2;
  for (const row of rungView(on, 0)) expect(row.verdict).not.toContain("standstill check off");
});

describe("comparison fields", () => {
  const scored = (over: Partial<RungVerdict> = {}) => ({ ...none, score: 1, deciding_speed: 1, ...over });
  const ladder = (over: Partial<LadderAssessment>): LadderAssessment => ({
    verdicts: [none, none, none, none],
    medians: [medians([]), medians([]), medians([]), medians([])],
    halt: null,
    best: null,
    runner_up: null,
    tie: [],
    spreads: [],
    ...over,
  });
  const withAssessment = (a: LadderAssessment) => tune("AwaitingChoice", [step({ assessment: a })]);

  test("rows carry the deciding speed, gap and tolerance, and mark the best", () => {
    const rows = rungView(
      withAssessment(
        ladder({
          best: 0,
          verdicts: [scored({ gap: 0 }), scored({ gap: 5.1, tolerance: 3.1 }), none, none],
        }),
      ),
      0,
    );
    expect(rows[0]).toMatchObject({ best: true, decidingSpeed: 1 });
    expect(rows[1]).toMatchObject({ best: false, gap: 5.1, tolerance: 3.1 });
  });

  test("rows beyond a halt carry no comparison fields", () => {
    const rows = rungView(
      withAssessment(
        ladder({ best: 0, halt: [1, "DampingUnassessable"], verdicts: [scored({ gap: 0 }), none, none, none] }),
      ),
      0,
    );
    expect(rows[2]).toMatchObject({ decidingSpeed: null, gap: null, tolerance: null, best: false });
  });

  test("the comparison line names the tie, its gap and tolerance, and the lower gain", () => {
    const text = comparisonText(
      ladder({
        best: 1,
        tie: [1, 2],
        verdicts: [none, scored({ gap: 0 }), scored({ gap: 2.0, tolerance: 3.1 }), none],
      }),
      3,
    );
    expect(text).toMatch(/rungs 2 and 3/);
    expect(text).toMatch(/2\.0/);
    expect(text).toMatch(/3\.1/);
    expect(text).toMatch(/lower gain/i);
  });

  test("a three-rung tie reports the largest gap in the set with that rung's tolerance", () => {
    const text = comparisonText(
      ladder({
        best: 1,
        tie: [0, 1, 2],
        verdicts: [
          scored({ gap: 2.5, tolerance: 3.3 }),
          scored({ gap: 0 }),
          scored({ gap: 1.0, tolerance: 3.0 }),
          none,
        ],
      }),
      3,
    );
    expect(text).toMatch(/rungs 1, 2 and 3/);
    expect(text).toMatch(/2\.5/);
    expect(text).toMatch(/3\.3/);
    expect(text).not.toMatch(/1\.0 μm/);
    expect(text).not.toMatch(/3\.0 μm/);
  });

  test("the comparison line names the runner-up when there is no tie", () => {
    const text = comparisonText(
      ladder({
        best: 1,
        tie: [1],
        runner_up: 2,
        verdicts: [none, scored({ gap: 0 }), scored({ gap: 5.1, tolerance: 3.1 }), none],
      }),
      3,
    );
    expect(text).toMatch(/rung 3/);
    expect(text).toMatch(/5\.1/);
    expect(text).toMatch(/3\.1/);
    expect(text).not.toMatch(/tie/i);
  });

  test("one repeat names the runner-up without a tolerance and says ties are undetectable", () => {
    const text = comparisonText(
      ladder({
        best: 1,
        tie: [1],
        runner_up: 2,
        verdicts: [none, scored({ gap: 0 }), scored({ gap: 0.1, tolerance: null }), none],
      }),
      1,
    );
    expect(text).toMatch(/rung 3/);
    expect(text).toMatch(/0\.1/);
    expect(text).toMatch(/one repeat/i);
    expect(text).not.toMatch(/0\.1 μm[^.]*\d\.\d μm/);
  });

  test("a single eligible rung says nothing was compared", () => {
    const text = comparisonText(ladder({ best: 0, runner_up: null, tie: [0] }), 3);
    expect(text).toMatch(/nothing to compare|only scored rung/i);
  });

  test("no best rung gives no comparison line", () => {
    expect(comparisonText(ladder({ best: null }), 3)).toBeNull();
  });
});

describe("driftLines", () => {
  test("report drifted and unassessed speeds and skip steady ones", () => {
    const lines = driftLines([
      { metric: "p99 50 mm/s", first: 59.2, rerun: 58.0, verdict: "Steady" },
      { metric: "p99 195 mm/s", first: 84.8, rerun: 80.6, verdict: "Drifted" },
      { metric: "p99 400 mm/s", first: 84.8, rerun: null, verdict: "Unassessed" },
      { metric: "zeta", first: 0.03, rerun: 0.03, verdict: null },
    ]);
    expect(lines).toHaveLength(2);
    expect(lines[0]).toMatch(/195 mm\/s/);
    expect(lines[0]).toMatch(/drift/i);
    expect(lines[1]).toMatch(/400 mm\/s/);
    expect(lines[1]).toMatch(/could not be assessed/i);
  });
});

describe("currentGuidedStep", () => {
  test("a tune with no step yet starts at its first axis", () => {
    expect(currentGuidedStep(tune("Ready", []))).toBe("velocity-p");
  });
  test("a step in progress shows its own axis", () => {
    for (const state of ["Announced", "Running", "Paused", "AwaitingChoice"] as const) {
      expect(currentGuidedStep(tune(state, [step({ axis: "VelocityICornerHz" })]))).toBe("velocity-i");
    }
  });
  test("a chosen step moves on to the next axis", () => {
    expect(currentGuidedStep(tune("Ready", [step({ axis: "VelocityP" })]))).toBe("velocity-i");
    expect(currentGuidedStep(tune("Ready", [step(), step({ axis: "VelocityICornerHz" })]))).toBe("position-p");
  });
  test("a finished or abandoned tune, or one with all three axes done, shows the result", () => {
    expect(currentGuidedStep(tune("Done"))).toBe("result");
    expect(currentGuidedStep(tune("Abandoned"))).toBe("result");
    const all = [step(), step({ axis: "VelocityICornerHz" }), step({ axis: "PositionP" })];
    expect(currentGuidedStep(tune("Ready", all))).toBe("result");
  });
  test("a locked tune stays on the axis it was working on", () => {
    expect(currentGuidedStep(tune("RestorePending", [step({ axis: "PositionP" })]))).toBe("position-p");
  });
});

describe("failureText", () => {
  const conflict = new ApiRequestError(409, "the tune is already over");
  const open = tune("Ready", []);
  test.each([
    ["abandon", "there is nothing to abandon"],
    ["setup", "its setup can't change any more"],
    ["start", "nothing more can be done with it"],
    ["restore", "there is nothing to restore"],
  ] as const)("a refused %s on an abandoned tune says why", (action, reason) => {
    const text = failureText(action, conflict, open, tune("Abandoned", []));
    expect(text).toContain("This tune was already abandoned");
    expect(text).toContain(reason);
    expect(text).toContain("The page now shows its current state.");
    expect(text).not.toContain("already over");
  });

  test("a finished tune is called finished", () => {
    expect(failureText("abandon", conflict, open, tune("Done"))).toContain("already finished");
  });

  test("a tune that moved on since the page last updated says so", () => {
    const text = failureText("setup", new ApiRequestError(409, "x"), open, tune("Announced"));
    expect(text).toMatch(/changed since the page last updated/);
    expect(text).not.toContain("x");
  });

  test("a refusal about the tune as it really is keeps the server's reason", () => {
    const reason = new ApiRequestError(409, "stepper_x is locked: restore pending");
    expect(failureText("start", reason, open, tune("Ready", []))).toBe("stepper_x is locked: restore pending");
  });

  test("a rejected value keeps its field and reason", () => {
    const bad = new ApiRequestError(400, "must be above zero", "seed");
    expect(failureText("prepare", bad, open, open)).toBe("seed: must be above zero");
  });

  test("a failure that is not an API error reads as a request failure", () => {
    expect(failureText("start", new Error("boom"), open, open)).toBe("boom");
    expect(failureText("start", "weird", null, null)).toBe("request failed");
  });
});
