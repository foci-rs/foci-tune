import { describe, expect, test } from "vitest";
import { ApiRequestError } from "./api";
import type { PrinterView, LadderAssessment, RungVerdict, Tune, TuneState, TuneStep } from "./api";
import {
  announcementOf,
  availableActions,
  curveData,
  canChoose,
  currentGuidedStep,
  failureText,
  DEFAULT_PREPARE,
  liveControlFor,
  liveSeed,
  prepareState,
  stepPercent,
  skipText,
  acceptGain,
  measuredGains,
  manualProblem,
  extensionOffer,
  fitReasonText,
  standstillCheckNote,
  sweepProgress,
  stepSentence,
  axisTitle,
  formFromStep,
  plannedRuns,
  previewLadder,
  defaultProbeSpeed,
  expectedDurationSeconds,
  nextAxis,
  recommendedGain,
  driftCheckRunning,
  rungView,
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
    octave_divisions: 3,
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
    expect(rows[1].verdict).toMatch(/damping too low/i);
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
    expect(rows[0].verdict).toBe("Scored");
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
    expect(rows[1].verdict).toMatch(/standstill noise too high/i);
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
    expect(rows[0].verdict).toBe("Skipped: step stopped");
    expect(rows[0].detail).toBe("run failed earlier");
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
    expect(rows[2].verdict).toBe("Stopped");
  });

  test("lists omitted and merged rungs as notes", () => {
    const s = step({ notes: [{ OutOfRange: 99999 }, { RoundedToZero: 0.1 }, { Merged: { shown: 1001, into: 1000 } }] });
    const notes = stepSummary(tune("Running", [s]), 0).notes;
    expect(notes).toEqual([
      "Gain 99999 was left out: it is outside what the stepper accepts.",
      "Gain 0 was left out: it rounds to zero.",
      "Gain 1001 was left out: it is the same as 1000.",
    ]);
  });
  test("a corner's left-out gains are in Hz", () => {
    const s = step({ axis: "VelocityICornerHz", notes: [{ OutOfRange: 250.04 }] });
    expect(stepSummary(tune("Running", [s]), 0).notes).toEqual([
      "Gain 250.0 Hz was left out: it is outside what the stepper accepts.",
    ]);
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
      `Paused at gain ${Math.round(t.steps[0].rungs[1].shown)}: the damping could not be measured.`,
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
    expect(rows[1].verdict).toContain("Continued: damping not measured");
    expect(rows[2].verdict).toBe("Paused: noise not measured");
    expect(rows[3].verdict).toBe("Paused");
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
  test("the standstill check being off is a note for the step, not part of every row", () => {
    const t = tune("Running");
    t.steps[0].standstill_gate_off = 5;
    expect(standstillCheckNote(t, 0)).toBe(
      "The standstill noise check is off for this step, so a gain is not rejected for noise at standstill.",
    );
    for (const row of rungView(t, 0)) expect(row.verdict).not.toMatch(/standstill/i);
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

test("the standstill note appears when the setup or the step has the check off, and only then", () => {
  const off = tune("AwaitingChoice", [step({ standstill_gate_off: 123 })]);
  off.setup.gates.standstill_factor = null;
  expect(standstillCheckNote(off, 0)).not.toBeNull();

  const setupOnly = tune("AwaitingChoice", [step()]);
  setupOnly.setup.gates.standstill_factor = null;
  expect(standstillCheckNote(setupOnly, 0)).not.toBeNull();

  const on = tune("AwaitingChoice", [step({ standstill_gate_off: null })]);
  on.setup.gates.standstill_factor = 2;
  expect(standstillCheckNote(on, 0)).toBeNull();
  for (const row of rungView(setupOnly, 0)) expect(row.verdict).not.toMatch(/standstill/i);
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
    expect(failureText("prepare", bad, open, open)).toBe("Seed: must be above zero");
  });

  test("a failure that is not an API error reads as a request failure", () => {
    expect(failureText("start", new Error("boom"), open, open)).toBe("boom");
    expect(failureText("start", "weird", null, null)).toBe("Something went wrong. Try again.");
  });
});

describe("liveSeed", () => {
  const control = { velocity_p: 1000, velocity_i: 50, position_p: 800, pwm_hz: 50000 };
  test("P gains are the live raw values", () => {
    expect(liveSeed("VelocityP", control)).toBe(1000);
    expect(liveSeed("PositionP", control)).toBe(800);
  });
  test("the I gain is shown as its corner frequency", () => {
    const hz = liveSeed("VelocityICornerHz", { ...control, velocity_i: 4200 })!;
    expect(hz).toBeCloseTo((4200 * 50000) / (2 * Math.PI * 65536), 9);
  });
  test("a corner cannot be shown while the PWM frequency is unknown", () => {
    expect(liveSeed("VelocityICornerHz", { ...control, pwm_hz: 0 })).toBeNull();
  });
});

describe("previewLadder", () => {
  const gainsOf = (r: ReturnType<typeof previewLadder>) => ("problem" in r ? [] : r.gains.map((g) => [g.step, g.raw]));

  test("a P ladder is the seed times 2^(k/3), rounded", () => {
    expect(gainsOf(previewLadder("VelocityP", 1000, 50000, -3, 3))).toEqual([
      [-3, 500],
      [-2, 630],
      [-1, 794],
      [0, 1000],
      [1, 1260],
      [2, 1587],
      [3, 2000],
    ]);
  });

  test("a step whose gain cannot be written is left out with its reason", () => {
    const r = previewLadder("VelocityP", 30000, 50000, 0, 3);
    if ("problem" in r) throw new Error(r.problem);
    expect(r.gains.map((g) => g.step)).toEqual([0]);
    expect(r.left.map((l) => l.step)).toEqual([1, 2, 3]);
    expect(r.left[0].reason).toMatch(/outside what the stepper accepts/);
  });

  test("steps that round to the same gain keep the first and name it", () => {
    const r = previewLadder("VelocityP", 4, 50000, -2, -1);
    if ("problem" in r) throw new Error(r.problem);
    expect(r.gains.map((g) => [g.step, g.raw])).toEqual([[-2, 3]]);
    expect(r.left).toEqual([
      expect.objectContaining({ step: -1, reason: expect.stringMatching(/same gain as step -2/) }),
    ]);
  });

  test("a gain that rounds to zero is left out", () => {
    const r = previewLadder("VelocityP", 1, 50000, -4, -4);
    if ("problem" in r) throw new Error(r.problem);
    expect(r.left.some((l) => /rounds to zero/.test(l.reason))).toBe(true);
  });

  test("the I corner ladder is in Hz and a zero seed stands in as 50 Hz", () => {
    const r = previewLadder("VelocityICornerHz", 0, 50000, 0, 0);
    if ("problem" in r) throw new Error(r.problem);
    expect(r.gains[0].shown).toBeCloseTo(50, 1);
  });

  test.each([
    ["VelocityP", 0, /can't be zero/],
    ["PositionP", 0, /can't be zero/],
    ["VelocityP", 0.2, /too small/],
    ["VelocityP", 40000, /outside what the stepper accepts/],
  ] as const)("a %s seed of %d is a problem with a sentence", (axis, seed, pattern) => {
    const r = previewLadder(axis, seed, 50000, -3, 3);
    expect("problem" in r && r.problem).toMatch(pattern);
  });

  test("an unknown PWM frequency makes a corner ladder impossible", () => {
    const r = previewLadder("VelocityICornerHz", 50, 0, -3, 3);
    expect("problem" in r && r.problem).toMatch(/PWM frequency/);
  });
});

describe("prepareState", () => {
  const control = { velocity_p: 1000, velocity_i: 50, position_p: 800, pwm_hz: 50000 };

  test("the seed follows the live value until one is typed, and the ladder follows the seed", () => {
    const live = prepareState("VelocityP", DEFAULT_PREPARE, control);
    expect(live).toMatchObject({ seed: 1000, problem: null });
    expect(live.preview?.gains).toHaveLength(7);
    const typed = prepareState("VelocityP", { ...DEFAULT_PREPARE, seed: 2000 }, control);
    expect(typed.seed).toBe(2000);
    expect(typed.preview?.gains[3].raw).toBe(2000);
  });

  test("without a live value a seed must be typed", () => {
    const none = prepareState("VelocityP", DEFAULT_PREPARE, null);
    expect(none.preview).toBeNull();
    expect(none.problem).toMatch(/no live value.*enter a seed/i);
    expect(prepareState("VelocityP", { ...DEFAULT_PREPARE, seed: 900 }, null).problem).toBeNull();
  });

  test("a form that cannot be sent says why, in this order: repeats, range, seed, ladder", () => {
    expect(prepareState("VelocityP", { ...DEFAULT_PREPARE, repeats: 0 }, control).problem).toMatch(
      /repeat at least once/,
    );
    expect(prepareState("VelocityP", { ...DEFAULT_PREPARE, lower: 2, upper: 1 }, control).problem).toMatch(
      /lowest.*first/i,
    );
    expect(prepareState("VelocityP", { ...DEFAULT_PREPARE, seed: 0 }, control).problem).toMatch(/can't be zero/);
    const unwritable = prepareState("VelocityP", { ...DEFAULT_PREPARE, seed: 32000, lower: 3, upper: 4 }, control);
    expect(unwritable.problem).toMatch(/none of these steps/i);
  });

  test("the I corner seed is in Hz with the live value converted", () => {
    const state = prepareState("VelocityICornerHz", DEFAULT_PREPARE, { ...control, velocity_i: 4200 });
    expect(state.unit).toBe("Hz");
    expect(state.seed).toBeCloseTo((4200 * 50000) / (2 * Math.PI * 65536), 6);
  });
});

describe("liveControlFor", () => {
  const printer = {
    steppers: [{ name: "stepper_x", serial: "s1", channel: 1 }],
    boards: [
      { serial: "s0", channels: [{ channel: 1, control: { velocity_p: 1 } }] },
      { serial: "s1", channels: [{ channel: 1, control: { velocity_p: 2, velocity_i: 3, position_p: 4, pwm_hz: 5 } }] },
    ],
  } as unknown as PrinterView;
  test("finds the control values of the stepper's own board and channel", () => {
    expect(liveControlFor(printer, "stepper_x")).toEqual({ velocity_p: 2, velocity_i: 3, position_p: 4, pwm_hz: 5 });
  });
  test("is null for an unknown stepper or a board that is not reporting", () => {
    expect(liveControlFor(printer, "stepper_y")).toBeNull();
    expect(liveControlFor({ ...printer, boards: [] } as unknown as PrinterView, "stepper_x")).toBeNull();
  });
});

describe("step size", () => {
  test("the offered divisions are 12%, 19%, 26% and 41% apart", () => {
    expect([6, 4, 3, 2].map(stepPercent)).toEqual([12, 19, 26, 41]);
  });

  test("a finer division spaces the same steps closer", () => {
    const r = previewLadder("VelocityP", 1000, 50000, -3, 3, 6);
    expect("problem" in r ? [] : r.gains.map((g) => g.raw)).toEqual([707, 794, 891, 1000, 1122, 1260, 1414]);
  });

  test("the form's division is what the ladder is built with", () => {
    const control = { velocity_p: 1000, velocity_i: 50, position_p: 800, pwm_hz: 50000 };
    const state = prepareState("VelocityP", { ...DEFAULT_PREPARE, divisions: 2, lower: -2, upper: 2 }, control);
    expect(state.preview?.gains.map((g) => g.raw)).toEqual([500, 707, 1000, 1414, 2000]);
  });
});

describe("stepSentence", () => {
  const p = (raws: number[], over: Partial<TuneStep> = {}) =>
    step({ seed_raw: 841, repeats: 3, rungs: raws.map((raw) => rung(0, { raw, shown: raw })), ...over });

  test("says how many gains, from where to where, around what, and how often each is tested", () => {
    expect(stepSentence(p([421, 530, 668, 841, 1060, 1335, 1682]))).toBe(
      "7 gains from 421 to 1682, around the starting gain of 841, each tested 3 times.",
    );
  });
  test("one repeat is tested once", () => {
    expect(stepSentence(p([421, 841], { repeats: 1 }))).toBe(
      "2 gains from 421 to 841, around the starting gain of 841, each tested once.",
    );
  });
  test("a single gain is not a range", () => {
    expect(stepSentence(p([841]))).toBe("1 gain, 841, tested 3 times.");
  });
  test("a corner ladder is in Hz, and a zero seed starts at 50 Hz", () => {
    const corner = step({
      axis: "VelocityICornerHz",
      seed_raw: 0,
      repeats: 2,
      rungs: [25, 50, 100].map((shown) => rung(0, { shown, raw: 1 })),
    });
    expect(stepSentence(corner)).toBe(
      "3 gains from 25.0 Hz to 100.0 Hz, around the starting gain of 50.0 Hz, each tested 2 times.",
    );
  });
});

describe("axisTitle", () => {
  test("names the axis without its unit", () => {
    expect(axisTitle("VelocityICornerHz")).toBe("Velocity I corner");
    expect(axisTitle("VelocityP")).toBe("Velocity P");
  });
});

describe("plannedRuns", () => {
  const preview = (range: [number, number]) => {
    const r = previewLadder("VelocityP", 841, 50000, range[0], range[1]);
    if ("problem" in r) throw new Error(r.problem);
    return r;
  };
  test("is the gains times the repeats times a recording per speed plus a probe and a standstill", () => {
    expect(plannedRuns(preview([1, 3]), 3, 3)).toBe(3 * 3 * 5);
  });
  test("adds one more set when the seed's own gain is in the ladder, for the drift check", () => {
    expect(plannedRuns(preview([-3, 3]), 3, 3)).toBe(7 * 3 * 5 + 5);
  });
  test("matches the service's own count for the default ladder", () => {
    expect(plannedRuns(preview([-3, 3]), 3, 3)).toBe(110);
  });
});

describe("formFromStep", () => {
  const s = step({ seed_raw: 841, repeats: 2, lower_k: -2, upper_k: 4, octave_divisions: 6 });
  test("takes the step's repeats, range and step size", () => {
    expect(formFromStep(s, 841)).toMatchObject({ repeats: 2, lower: -2, upper: 4, divisions: 6 });
  });
  test("follows the live value when the seed is still it, and types the seed when it is not", () => {
    expect(formFromStep(s, 841).seed).toBeNull();
    expect(formFromStep(s, 900).seed).toBe(841);
    expect(formFromStep(s, null).seed).toBe(841);
  });
  test("a corner seed is read back in Hz", () => {
    const corner = step({ axis: "VelocityICornerHz", seed_raw: 4200, pwm_hz: 50000 });
    const hz = (4200 * 50000) / (2 * Math.PI * 65536);
    expect(formFromStep(corner, hz).seed).toBeNull();
    expect(formFromStep(corner, 10)?.seed).toBeCloseTo(hz, 6);
  });
});

describe("the status of a rung that is being run", () => {
  const running = (rungOver: Partial<ReturnType<typeof rung>>) => {
    const t = tune("Running", [step({ repeats: 3, rungs: [rung(0, rungOver), rung(1), rung(2), rung(3)] })]);
    return rungView(t, 0);
  };
  test("a rung whose runs have started says how many of its runs are done, not Pending", () => {
    const rows = running({ started_unix_ms: 1, run_ids: ["a", "b"] });
    expect(rows[0].verdict).toMatch(/^Running: 2 of 12 runs/);
    expect(rows[1].verdict).toMatch(/^Pending/);
  });
  test("the first run of a rung is already Running with none done", () => {
    expect(running({ started_unix_ms: 1 })[0].verdict).toMatch(/^Running: 0 of 12 runs/);
  });
  test("a rung that has ended is not shown as running", () => {
    const rows = running({ started_unix_ms: 1, ended_unix_ms: 2, run_ids: Array(12).fill("r") });
    expect(rows[0].verdict).not.toMatch(/Running/);
  });
});

describe("sweepProgress", () => {
  const time = (id: string) => ({ id, started_unix_ms: 0, ended_unix_ms: 1 });
  test("counts the runs finished across the rungs and the drift check against the service's total", () => {
    const t = tune("Running", [
      step({
        run_count: 110,
        rungs: [rung(0, { run_times: [time("a"), time("b")] }), rung(1, { run_times: [time("c")] })],
        drift_rung: rung(0, { run_times: [time("d")] }),
      }),
    ]);
    expect(sweepProgress(t, 0)).toEqual({ done: 4, total: 110 });
  });
  test("works out the total itself before the service has given one", () => {
    const t = tune("Running", [
      step({ run_count: null, repeats: 2, rungs: [rung(0, { raw: 1000 }), rung(1, { raw: 1100 })], seed_raw: 1000 }),
    ]);
    expect(sweepProgress(t, 0)).toEqual({ done: 0, total: 2 * 2 * 4 + 4 });
  });
});

describe("paused and stopped rows", () => {
  const paused = (reason: "DampingUnassessable" | "StandstillBaselineUnassessable") => {
    const t = tune("Paused");
    t.steps[0].paused = { rung: 1, reason, unix_ms: 1 };
    t.steps[0].assessment = {
      verdicts: [none, none, none, none],
      medians: [medians([]), medians([]), medians([]), medians([])],
      halt: null,
      pause: [1, reason],
      best: null,
      runner_up: null,
      tie: [],
      spreads: [],
    };
    return rungView(t, 0).map((r) => r.verdict);
  };
  test("the paused rung says why in a few words and the rungs after it just say Paused", () => {
    expect(paused("DampingUnassessable").slice(1)).toEqual(["Paused: damping not measured", "Paused", "Paused"]);
    expect(paused("StandstillBaselineUnassessable")[1]).toBe("Paused: noise not measured");
  });
  test("a gain is a whole number, and a corner gain is in Hz", () => {
    const t = tune("Paused", [
      step({ axis: "VelocityICornerHz", rungs: [rung(0, { shown: 25.04 }), rung(1, { shown: 31.5 })] }),
    ]);
    t.steps[0].paused = { rung: 1, reason: "DampingUnassessable", unix_ms: 1 };
    t.steps[0].assessment = {
      verdicts: [none, none],
      medians: [medians([]), medians([])],
      halt: null,
      pause: [1, "DampingUnassessable"],
      best: null,
      runner_up: null,
      tie: [],
      spreads: [],
    };
    expect(stepSummary(t, 0).pause).toBe("Paused at gain 31.5 Hz: the damping could not be measured.");
  });
});

describe("fitReasonText", () => {
  test("rounds the frequency and says it in the words used elsewhere", () => {
    expect(fitReasonText("damped frequency 34.73666787130117 Hz is outside 60 to 490 Hz")).toBe(
      "The ringing frequency, 34.7 Hz, is outside the 60 to 490 Hz the fit accepts.",
    );
  });
  test("any other reason keeps its words but loses the long decimals", () => {
    expect(fitReasonText("residual 0.123456789 too high")).toBe("residual 0.123 too high");
    expect(fitReasonText("no probe record")).toBe("no probe record");
  });
});

describe("skipText", () => {
  test("a gain not run because damping was too low is short, with the numbers as the detail", () => {
    expect(skipText("not run: median damping 0.011 is below the floor 0.02")).toEqual({
      text: "damping too low",
      detail: "Median damping 0.011 was below the stop limit of 0.02.",
    });
  });
  test("damping or standstill noise that could not be measured", () => {
    expect(skipText("not run: damping unassessable").text).toBe("damping not measured");
    expect(skipText("not run: standstill baseline unassessable").text).toBe("noise not measured");
  });
  test("any other reason is a stopped step with the service's words as the detail", () => {
    expect(skipText("not run: run refused: the printer is printing")).toEqual({
      text: "step stopped",
      detail: "run refused: the printer is printing",
    });
  });
});

describe("the status of a scored gain", () => {
  test("is Scored, and the recommended measured gain reads Recommended", () => {
    const t = tune("AwaitingChoice", [
      step({
        assessment: {
          verdicts: [none, { ...none, score: 5 }, { ...none, score: 3 }, { ...none, score: 9 }],
          medians: [medians([]), medians([4]), medians([3]), medians([9])],
          halt: null,
          best: 2,
          runner_up: 1,
          tie: [2],
          spreads: [],
        },
        recommendation: { choice: { Measured: { rung: 2 } }, extend: null },
      }),
    ]);
    const rows = rungView(t, 0);
    expect(rows[1].verdict).toBe("Scored");
    expect(rows[2].verdict).toBe("Recommended");
    expect(rows[3].verdict).toBe("Scored");
  });
});

describe("choosing a gain", () => {
  const done = { zeta: { Assessed: 0.1 }, p99_by_speed: [3, 4], worst_torque_sd: 1 };
  const finished = (choice: NonNullable<TuneStep["recommendation"]>["choice"], over: Partial<TuneStep> = {}) =>
    tune("AwaitingChoice", [
      step({
        repeats: 1,
        seed_raw: 841,
        rungs: [
          rung(0, { raw: 421, shown: 421, repeat_values: [done] }),
          rung(1, { raw: 530, shown: 530, repeat_values: [done] }),
          rung(2, { raw: 668, shown: 668, skipped: "not run: damping unassessable" }),
        ],
        assessment: {
          verdicts: [none, none, none],
          medians: [medians([]), medians([]), medians([])],
          halt: null,
          best: 1,
          runner_up: null,
          tie: [1],
          spreads: [],
        },
        recommendation: { choice, extend: null },
        ...over,
      }),
    ]);

  test("Accept names the recommended gain: a measured one, or an interpolated value", () => {
    expect(acceptGain(finished({ Measured: { rung: 1 } }), 0)).toBe("530");
    expect(acceptGain(finished({ Interpolated: { shown: 480.4, between: [0, 1] } }), 0)).toBe("480");
  });
  test("with no recommendation there is nothing to accept", () => {
    expect(acceptGain(finished({ None: "NoScoredRung" }), 0)).toBeNull();
    expect(acceptGain(tune("Running", [step()]), 0)).toBeNull();
  });
  test("a corner's accepted gain is in Hz", () => {
    const t = finished(
      { Measured: { rung: 0 } },
      { axis: "VelocityICornerHz", rungs: [rung(0, { raw: 4200, shown: 50.04, repeat_values: [done] })] },
    );
    expect(acceptGain(t, 0)).toBe("50.0 Hz");
  });

  test("the dropdown lists the gains that finished measuring, with the recommended one marked", () => {
    expect(measuredGains(finished({ Measured: { rung: 1 } }), 0)).toEqual([
      { index: 0, label: "421", recommended: false },
      { index: 1, label: "530", recommended: true },
    ]);
  });
  test("an interpolated recommendation marks no gain of the list", () => {
    expect(
      measuredGains(finished({ Interpolated: { shown: 480, between: [0, 1] } }), 0).some((g) => g.recommended),
    ).toBe(false);
  });

  test.each([
    ["VelocityP", 1060, null],
    ["VelocityP", 0, /above zero/],
    ["VelocityP", -5, /above zero/],
    ["VelocityP", 40000, /outside what the stepper accepts/],
    ["VelocityP", Number.NaN, /number/],
    ["PositionP", 800.4, null],
  ] as const)("a typed %s gain of %s: %s", (axis, value, problem) => {
    const result = manualProblem(axis, value, 50000);
    if (problem === null) expect(result).toBeNull();
    else expect(result).toMatch(problem);
  });
  test("a corner needs the PWM frequency and stays within what the stepper accepts", () => {
    expect(manualProblem("VelocityICornerHz", 50, 50000)).toBeNull();
    expect(manualProblem("VelocityICornerHz", 50, 0)).toMatch(/PWM frequency/);
    expect(manualProblem("VelocityICornerHz", 100000, 50000)).toMatch(/outside what the stepper accepts/);
  });
});

describe("extensionOffer", () => {
  const speeds2 = (extend: "Below" | "Above" | null, bestRung: number) => {
    const t = tune("AwaitingChoice", [
      step({
        repeats: 3,
        seed_raw: 841,
        lower_k: -3,
        upper_k: 3,
        octave_divisions: 3,
        rungs: [
          rung(0, { raw: 421, shown: 421 }),
          rung(1, { raw: 530, shown: 530 }),
          rung(2, { raw: 668, shown: 668 }),
        ],
        assessment: {
          verdicts: [none, none, none],
          medians: [medians([]), medians([]), medians([])],
          halt: null,
          best: bestRung,
          runner_up: null,
          tie: [bestRung],
          spreads: [],
        },
        recommendation: { choice: { Measured: { rung: bestRung } }, extend: extend },
      }),
    ]);
    return t;
  };
  test("nothing to offer when the best gain is not at an edge", () => {
    expect(extensionOffer(speeds2(null, 1), 0)).toBeNull();
  });
  test("a best gain at the low end offers a lower one, with its gain and the runs it adds", () => {
    expect(extensionOffer(speeds2("Below", 0), 0)).toEqual({
      direction: "below",
      gain: "334",
      runs: 12,
      text: "The best gain, 421, is at the low end of what was tried, so a lower gain may be better. Try 334: 12 more runs.",
    });
  });
  test("a best gain at the high end offers a higher one", () => {
    const offer = extensionOffer(speeds2("Above", 2), 0)!;
    expect(offer.direction).toBe("above");
    expect(offer.gain).toBe("2119");
    expect(offer.text).toMatch(/high end.*a higher gain may be better/);
  });
});
