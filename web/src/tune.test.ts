import { describe, expect, test } from "vitest";
import { ApiRequestError, type LiveChannel, type Pattern, type Run, type RunKind, type SessionView } from "./api";
import {
  alignSeries,
  appendLive,
  assignColors,
  buildLadderRequest,
  clampPoint,
  DEFAULT_PARAMETERS,
  defaultLadderValues,
  headlineMetric,
  ladderFieldErrors,
  queueInOrder,
  runProgress,
  skipReasonText,
  sweepRejectionNotes,
  runPosition,
  sweepOrder,
  ladderMedians,
  ladderPlotData,
  patternProblem,
  expectedProbeEdges,
  patternIsolation,
  patternMode,
  probeEdgeHint,
  suggestedProbeStrokes,
  probeToggleSpacing,
  segmentTravelMm,
  runStatusLabel,
  toggleSpacingProblem,
  isUnknownSession,
  pickRestoreSession,
  restoreStillApplies,
  sessionForRun,
  liveRegime,
  loadPattern,
  nextSession,
  parseLadderList,
  savePattern,
  setupFromKind,
  type RunBody,
  type Setup,
  type LadderInput,
} from "./tune";

const pattern = { start: [100, 100] as [number, number], end: [200, 200] as [number, number] };

function tracking(over: Partial<Setup> = {}): Setup {
  return {
    stepper: "stepper_x",
    pattern,
    type: "tracking",
    parameters: { speeds_mm_s: [50, 195, 400], strokes: 8 },
    ...over,
  } as Setup;
}

describe("nextSession", () => {
  test("is false for an identical setup", () => {
    expect(nextSession(tracking(), tracking())).toBe(false);
  });

  test("is true when the stepper differs", () => {
    expect(nextSession(tracking(), tracking({ stepper: "stepper_y" }))).toBe(true);
  });

  test("is true when the run type differs", () => {
    const probe: Setup = {
      stepper: "stepper_x",
      pattern,
      type: "probe",
      parameters: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 },
    };
    expect(nextSession(tracking(), probe)).toBe(true);
  });

  test("is true when the pattern differs", () => {
    const moved = { start: [100, 100] as [number, number], end: [200, 201] as [number, number] };
    expect(nextSession(tracking(), tracking({ pattern: moved }))).toBe(true);
  });

  test("is true when a parameter differs", () => {
    const b = tracking({ parameters: { speeds_mm_s: [50, 195], strokes: 8 } } as Partial<Setup>);
    expect(nextSession(tracking(), b)).toBe(true);
  });
});

describe("parseLadderList", () => {
  test("accepts commas with or without spaces", () => {
    expect(parseLadderList("100, 120,140")).toEqual({ ok: true, values: [100, 120, 140] });
  });

  test("rejects a non-number and names it", () => {
    const r = parseLadderList("a");
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toContain("a");
  });

  test("rejects an empty list", () => {
    expect(parseLadderList(" , ").ok).toBe(false);
  });
});

describe("headlineMetric", () => {
  function run(kind: RunKind, channel: Record<string, unknown>): Run {
    return {
      kind,
      declared_channel: 0,
      metrics: {
        channels: [
          { channel: 1, declared: false, tracking: [], probe: null, standstill: null },
          { channel: 0, declared: true, tracking: [], probe: null, standstill: null, ...channel },
        ],
      },
    } as unknown as Run;
  }

  test("tracking: p99 at the highest speed of the declared channel", () => {
    const rec = (speed_mm_s: number, p99_um: number | null) => ({
      speed_mm_s,
      cruise_mean_abs_um: 1,
      sd_um: 1,
      p99_um,
      reversal_peak_um: null,
      reversal_count: 0,
      moving_samples: 10,
    });
    const r = run(
      { Tracking: { speeds_mm_s: [50, 400], strokes: 8 } },
      {
        tracking: [rec(400, 33), rec(50, 12)],
      },
    );
    expect(headlineMetric(r, "tracking")).toMatchObject({ value: 33, unit: "μm" });
  });

  test("probe: velocity zeta, or no value with the fit's reason", () => {
    const fit = {
      status: "fit",
      damped_hz: 150,
      zeta: 0.12,
      amplitude: 1,
      phase: 0,
      offset: 0,
      tau_s: 0.01,
      residual_rms: 0.1,
    };
    const failed = { status: "failed", reason: "did not converge" };
    const probe = (velocity: unknown) => ({
      probe: {
        velocity,
        torque: failed,
        position_error: failed,
        edges: 30,
        stderr_at_peak: null,
        torque_psd_peak_hz: null,
      },
    });
    const kind: RunKind = {
      Probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 },
    };
    expect(headlineMetric(run(kind, probe(fit)), "probe")).toMatchObject({ value: 0.12 });
    expect(headlineMetric(run(kind, probe(failed)), "probe")).toMatchObject({
      value: null,
      note: "did not converge",
    });
  });

  test("standstill: worst torque sd", () => {
    const r = run(
      { Standstill: { rests: 8, dwell_s: 3 } },
      {
        standstill: { rests: [], worst_torque_sd: 41.5, worst_large_transitions_per_s: null },
      },
    );
    expect(headlineMetric(r, "standstill")).toMatchObject({ value: 41.5 });
    const withRate = run(
      { Standstill: { rests: 8, dwell_s: 3 } },
      {
        standstill: { rests: [], worst_torque_sd: 41.5, worst_large_transitions_per_s: 12.34 },
      },
    );
    expect(headlineMetric(withRate, "standstill")).toMatchObject({ value: 41.5, note: "large steps 12.3/s" });
    expect(headlineMetric(r, "standstill").note).toBeUndefined();
  });

  test("no metrics gives no value", () => {
    const r = { declared_channel: 0, metrics: null } as unknown as Run;
    expect(headlineMetric(r, "probe").value).toBeNull();
  });
});

describe("buildLadderRequest", () => {
  const base: RunBody = {
    session: null,
    stepper: "stepper_x",
    type: "probe",
    pattern,
    parameters: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 },
    gains: { velocity_p: 841, velocity_i: 412, position_p: 141 },
    filters: { velocity_hz: 500, torque_hz: 1000, position_hz: 500, flux_hz: 1000 },
  };

  test("never names a session: a ladder opens its own session that records the ladder", () => {
    const r = buildLadderRequest(
      { ...base, session: "20261031T085136Z-r87j" },
      { axis: "position_p", text: "100", repeats: 2, floor: 0.05, marginal: 0.08 },
      50_000,
    );
    expect(r.ok).toBe(true);
    if (r.ok) expect("session" in r.body).toBe(false);
  });

  test("carries the axis, values, repeats and thresholds", () => {
    const r = buildLadderRequest(
      base,
      { axis: "position_p", text: "100, 120", repeats: 2, floor: 0.05, marginal: 0.08 },
      50_000,
    );
    expect(r).toEqual({
      ok: true,
      body: {
        stepper: base.stepper,
        type: base.type,
        pattern: base.pattern,
        parameters: base.parameters,
        gains: base.gains,
        filters: base.filters,
        axis: "position_p",
        values: [100, 120],
        repeats: 2,
        floor: 0.05,
        marginal: 0.08,
      },
    });
  });

  test("rejects a value outside the raw range with the value shown", () => {
    const r = buildLadderRequest(
      base,
      { axis: "velocity_p", text: "800, 40000", repeats: 2, floor: 0.05, marginal: 0.08 },
      50_000,
    );
    expect(r).toEqual({ ok: false, error: "40000 converts to raw 40000, outside 0..=32767" });
  });

  test("converts a corner to raw through the PWM frequency for the range check", () => {
    const r = buildLadderRequest(
      base,
      { axis: "velocity_i_corner_hz", text: "50, 5000", repeats: 1, floor: 0.05, marginal: 0.08 },
      50_000,
    );
    expect(r).toEqual({ ok: false, error: "5000 converts to raw 41177, outside 0..=32767" });
  });

  test("refuses a corner ladder without a PWM frequency", () => {
    const r = buildLadderRequest(
      base,
      { axis: "velocity_i_corner_hz", text: "50", repeats: 1, floor: 0.05, marginal: 0.08 },
      null,
    );
    expect(r.ok).toBe(false);
  });

  test("refuses more runs than a ladder may queue", () => {
    const text = Array.from({ length: 20 }, (_, k) => 100 + k).join(",");
    const r = buildLadderRequest(base, { axis: "velocity_p", text, repeats: 4, floor: 0.05, marginal: 0.08 }, 50_000);
    expect(r).toEqual({ ok: false, error: "a ladder queues at most 60 runs; this one would queue 80" });
  });
});

describe("defaultLadderValues", () => {
  test("velocity P is two steps below and four above the current value, in ascending order", () => {
    expect(defaultLadderValues("velocity_p", { velocity_p: 841, velocity_i: 0, position_p: 0 }, 50_000)).toEqual([
      641, 741, 941, 1041, 1141, 1241,
    ]);
    expect(defaultLadderValues("velocity_p", { velocity_p: 150, velocity_i: 0, position_p: 0 }, 50_000)).toEqual([
      50, 250, 350, 450, 550,
    ]);
  });

  test("omit rungs outside the raw range", () => {
    expect(defaultLadderValues("position_p", { velocity_p: 0, velocity_i: 0, position_p: 10 }, 50_000)).toEqual([
      10, 30, 50, 70, 90,
    ]);
    expect(defaultLadderValues("velocity_p", { velocity_p: 32500, velocity_i: 0, position_p: 0 }, 50_000)).toEqual([
      32300, 32400, 32600, 32700,
    ]);
    expect(
      defaultLadderValues("velocity_i_corner_hz", { velocity_p: 0, velocity_i: 0, position_p: 0 }, 50_000),
    ).toEqual([25, 50, 75, 100]);
  });
});

describe("live conversion", () => {
  const channel = (ticks: number[], tmc: number[], pe: number[]): LiveChannel => ({
    channel: 0,
    ticks,
    step_position: tmc,
    tmc_position: tmc,
    position_error: pe,
    torque: ticks.map(() => 7),
    ff_offset: ticks.map(() => 0),
  });

  test("uses the batch clock and the rotation distance, not fixed constants", () => {
    const regime = { clock_freq: 1_000_000, motor_rotation_distance_mm: 40 };
    const trace = appendLive(null, channel([5000, 6000], [0, 65536 / 40], [65536 / 40, 0]), regime, 10);
    expect(trace.t_s).toEqual([0, 0.001]);
    expect(trace.position_error_um).toEqual([1000, 0]);
    expect(trace.velocity_mm_s[0]).toBeNull();
    expect(trace.velocity_mm_s[1]).toBeCloseTo(1000);
    expect(trace.torque).toEqual([7, 7]);
  });

  test("continues velocity across batches and drops samples outside the window", () => {
    const regime = { clock_freq: 1000, motor_rotation_distance_mm: 65.536 };
    let trace = appendLive(null, channel([0, 1000], [0, 1000], [0, 0]), regime, 1.5);
    trace = appendLive(trace, channel([2000], [3000], [0]), regime, 1.5);
    expect(trace.t_s).toEqual([1, 2]);
    expect(trace.velocity_mm_s).toEqual([1, 2]);
  });

  test("prefers the batch clock, falls back to the run regime, refuses an unknown clock", () => {
    const stepper = { motor_rotation_distance_mm: 40 };
    expect(liveRegime(260e6, null, stepper)).toEqual({
      ok: true,
      regime: { clock_freq: 260e6, motor_rotation_distance_mm: 40 },
    });
    const run = { clock_freq: 170e6, motor_rotation_distance_mm: 32, encoder_counts_per_rev: 4000, pwm_hz: 50_000 };
    expect(liveRegime(0, run, stepper)).toEqual({
      ok: true,
      regime: { clock_freq: 170e6, motor_rotation_distance_mm: 32 },
    });
    expect(liveRegime(0, null, stepper).ok).toBe(false);
  });
});

describe("alignSeries", () => {
  test("joins x values, keeps nulls as gaps and marks missing samples undefined", () => {
    const data = alignSeries([
      [
        [0, 1, 2],
        [10, null, 12],
      ],
      [
        [0.5, null, 2],
        [20, 21, 22],
      ],
    ]);
    expect(data).toEqual([
      [0, 0.5, 1, 2],
      [10, undefined, null, 12],
      [undefined, 20, undefined, 22],
    ]);
  });
});

describe("ladderPlotData", () => {
  test("groups repeats by value; no median for a value short of its repeats", () => {
    const e = (value: number, metric: number | null) => ({ value, metric, assessable: metric !== null });
    const data = ladderPlotData([e(120, 0.2), e(100, 0.3), e(120, null), e(100, 0.1), e(100, 0.5)], 3);
    expect(data).toEqual([
      [100, 120],
      [0.3, 0.2],
      [0.1, null],
      [0.5, null],
      [0.3, null],
    ]);
  });
});

describe("ladderMedians", () => {
  test("a not comparable repeat makes the value unassessable, as the engine's stop rule does", () => {
    const medians = ladderMedians(
      [
        { value: 100, metric: 0.2, assessable: true },
        { value: 200, metric: 0.3, assessable: true },
        { value: 100, metric: 0.25, assessable: false },
        { value: 200, metric: 0.1, assessable: true },
      ],
      2,
    );
    expect(medians).toEqual([
      { value: 100, median: null, unassessable: true },
      { value: 200, median: 0.2, unassessable: false },
    ]);
  });

  test("only the first repeats count, in run order", () => {
    const medians = ladderMedians(
      [
        { value: 100, metric: 0.2, assessable: true },
        { value: 100, metric: 0.4, assessable: true },
        { value: 100, metric: null, assessable: false },
      ],
      2,
    );
    expect(medians).toEqual([{ value: 100, median: 0.30000000000000004, unassessable: false }]);
  });
});

describe("session continuity", () => {
  const link = { id: "S1", setup: tracking() };

  test("a single run reuses the session while the setup is unchanged", () => {
    expect(sessionForRun(link, tracking())).toBe("S1");
    expect(sessionForRun(link, tracking({ stepper: "stepper_y" }))).toBeNull();
    expect(sessionForRun(null, tracking())).toBeNull();
  });

  function view(id: string, stepper: string): SessionView {
    return {
      id,
      tune: null,
      error: null,
      runs: [],
      session: {
        format_version: 1,
        name: id,
        created_unix_s: 0,
        stepper,
        kind: { Tracking: { speeds_mm_s: [50, 195, 400], strokes: 8 } },
        pattern,
        gains: { velocity_p: 1, velocity_i: 1, position_p: 1, position_i: 0 },
        filters: { velocity_hz: 1, torque_hz: 1, position_hz: 1, flux_hz: 1 },
        ladder: null,
      },
    };
  }

  test("restores the session of the run the engine is executing", () => {
    expect(pickRestoreSession("S9", [], [view("S1", "stepper_x")])).toBe("S9");
  });

  test("restores the session holding a queued run when nothing executes", () => {
    const queued = view("S2", "stepper_x");
    queued.runs = [{ id: "R7", state: "queued", run: null }];
    expect(pickRestoreSession(null, ["R7"], [view("S1", "stepper_x"), queued])).toBe("S2");
  });

  test("an idle engine restores nothing, however well a stored session matches", () => {
    expect(pickRestoreSession(null, [], [view("S1", "stepper_x")])).toBeNull();
  });

  test("a restore result is dropped when the link or the form changed meanwhile", () => {
    const before = { link: null, form: "setup-a" };
    expect(restoreStillApplies(before, { link: null, form: "setup-a" })).toBe(true);
    expect(restoreStillApplies(before, { link: "S5", form: "setup-a" })).toBe(false);
    expect(restoreStillApplies(before, { link: null, form: "setup-b" })).toBe(false);
  });

  test("recognises a linked session that no longer exists", () => {
    expect(isUnknownSession(new ApiRequestError(404, "session S1 not found"))).toBe(true);
    expect(isUnknownSession(new ApiRequestError(409, "unknown session S1"))).toBe(true);
    expect(isUnknownSession(new ApiRequestError(409, "a run is in flight"))).toBe(false);
    expect(isUnknownSession(new Error("network"))).toBe(false);
  });
});

describe("assignColors", () => {
  test("keeps a run's slot when another is unchecked and fills the lowest free slot", () => {
    let slots = assignColors(new Map(), ["a", "b", "c"]);
    expect([...slots]).toEqual([
      ["a", 0],
      ["b", 1],
      ["c", 2],
    ]);
    slots = assignColors(slots, ["a", "c"]);
    expect([...slots]).toEqual([
      ["a", 0],
      ["c", 2],
    ]);
    slots = assignColors(slots, ["a", "c", "d"]);
    expect(slots.get("d")).toBe(1);
  });
});

describe("pattern helpers", () => {
  const limits = { x: [0, 300] as [number, number], y: [0, 250] as [number, number] };

  test("names the limit an out-of-margin point breaks, and accepts a valid pattern", () => {
    expect(patternProblem(pattern, limits)).toBeNull();
    const typed = { start: [1, 100] as [number, number], end: [200, 200] as [number, number] };
    expect(patternProblem(typed, limits)).toBe("start X 1 is outside 10..290 mm (bed limits minus the 10 mm margin)");
    const short = { start: [100, 100] as [number, number], end: [100.5, 100] as [number, number] };
    expect(patternProblem(short, limits)).toBe("the pattern is shorter than 1 mm");
  });

  test("clamps a point to the limits minus the margin", () => {
    expect(clampPoint([-5, 400], limits)).toEqual([10, 240]);
  });

  test("remembers the last pattern per stepper and survives a throwing storage", () => {
    const store = new Map<string, string>();
    const storage = {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
    };
    savePattern(storage, "stepper_x", pattern);
    expect(loadPattern(storage, "stepper_x")).toEqual(pattern);
    expect(loadPattern(storage, "stepper_y")).toBeNull();
    const broken = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    };
    expect(loadPattern(broken, "stepper_x")).toBeNull();
    expect(() => savePattern(broken, "stepper_x", pattern)).not.toThrow();
    store.set("foci-tune.pattern.stepper_z", '{"start":[1]}');
    expect(loadPattern(storage, "stepper_z")).toBeNull();
  });

  test("reads a session's kind back into a setup", () => {
    expect(setupFromKind("stepper_x", pattern, { Standstill: { rests: 4, dwell_s: 2 } })).toEqual({
      stepper: "stepper_x",
      pattern,
      type: "standstill",
      parameters: { rests: 4, dwell_s: 2 },
    });
  });
});

describe("probe toggle spacing", () => {
  const diagonal: Pattern = { start: [100.5, 100.5], end: [200.5, 200.5] };
  const alongX: Pattern = { start: [100.5, 150.5], end: [200.5, 150.5] };
  const at = (speed: number, segments = 3, p = diagonal, kinematics = "corexy") =>
    probeToggleSpacing(p, kinematics, speed, segments, 500);

  test("a corexy diagonal segment is 10 mm of stepper travel, a cartesian one 5 mm", () => {
    expect(segmentTravelMm(diagonal, "corexy")).toBeCloseTo(10, 9);
    expect(segmentTravelMm(alongX, "cartesian")).toBeCloseTo(5, 9);
    expect(segmentTravelMm(diagonal, "delta")).toBeNull();
  });

  test.each([
    [50, 600, true, 1],
    [195, 153.8, true, 2],
    [400, 75, false, 4],
    [600, 50, false, 6],
  ])("corexy at %d mm/s with 3 segments: %d ms apart", (speed, ms, ok, minSegments) => {
    const s = at(speed)!;
    expect(s.spacingS * 1000).toBeCloseTo(ms, 1);
    expect(s.ok).toBe(ok);
    expect(s.minSegments).toBe(minSegments);
  });

  test("cartesian toggles at the same speed come twice as often", () => {
    expect(at(400, 3, alongX, "cartesian")!.spacingS * 1000).toBeCloseTo(37.5, 6);
  });

  test("the advised count is one the step-response estimate accepts", () => {
    const advised = at(400)!.cleanSegments;
    expect(advised).toBe(6);
    const spacing = at(400, advised)!;
    expect(spacing.ok).toBe(true);
    expect(spacing.spacingS).toBeGreaterThanOrEqual(0.15 - 1e-9);
  });

  test("exactly the minimum passes, just below it does not", () => {
    expect(at(400, 4)!.ok).toBe(true);
    expect(at(401, 4)!.ok).toBe(false);
    expect(at(401, 4)!.minSegments).toBe(5);
  });

  test("the estimate uses the speed after the max_velocity clamp", () => {
    const s = probeToggleSpacing(alongX, "cartesian", 700, 10, 500)!;
    expect(s.speedMmS).toBe(500);
    expect(s.ok).toBe(true);
  });

  test("the run is blocked with the spacing, the minimum and the segments to use", () => {
    const problem = toggleSpacingProblem(at(400)!);
    expect(problem).toContain("400 mm/s");
    expect(problem).toContain("75 ms");
    expect(problem).toContain("100 ms");
    expect(problem).toContain("150 ms");
    expect(problem).toContain("6 or more moves between gain changes");
    expect(toggleSpacingProblem(at(195)!)).toBeNull();
  });
});

describe("expected probe edges", () => {
  const diag = (reach: number, from = 50): Pattern => ({ start: [from, from], end: [from + reach, from + reach] });
  const recorded: Pattern = { start: [100.5, 104], end: [200.5, 204] };
  const edges = (speed: number, segments: number, p: Pattern, strokes = 8, kinematics = "corexy") =>
    expectedProbeEdges({
      pattern: p,
      kinematics,
      speedMmS: speed,
      segmentsBetweenToggles: segments,
      strokes,
      maxVelocity: 500,
    });

  test.each([
    ["195 mm/s, 3 segments, 100 mm", 195, 3, recorded, [40, 41, 42, 41, 41]],
    ["400 mm/s, 6 segments, 100 mm (operator report)", 400, 6, recorded, [19]],
    ["400 mm/s, 5 segments, 100 mm", 400, 5, recorded, [8]],
    ["400 mm/s, 5 segments, 200 mm", 400, 5, diag(200), [22]],
    ["600 mm/s, 6 segments, 200 mm", 600, 6, diag(200), [6]],
    ["600 mm/s, 10 segments, 200 mm", 600, 10, diag(200), [19]],
    ["600 mm/s, 10 segments, 260 mm", 600, 10, diag(260, 20), [29]],
    ["800 mm/s clamped, 10 segments, 260 mm", 800, 10, diag(260, 20), [23]],
  ])("%s: never above the fewest recorded edges, at most 7 below", (_, speed, segments, p, seen) => {
    const fewest = Math.min(...seen);
    const estimate = edges(speed, segments, p)!;
    expect(estimate).toBeLessThanOrEqual(fewest);
    expect(estimate).toBeGreaterThanOrEqual(fewest - 7);
  });

  test("the 195 mm/s reference reproduces within 3 of every recorded run", () => {
    expect(edges(195, 3, recorded)).toBeGreaterThanOrEqual(39);
  });

  test("more strokes or a longer pattern never lowers the estimate", () => {
    const byStrokes = [1, 4, 8, 16, 32, 50].map((n) => edges(400, 6, diag(100), n)!);
    const bySpan = [50, 100, 150, 200, 250].map((r) => edges(400, 6, diag(r))!);
    for (const series of [byStrokes, bySpan]) {
      for (const [i, v] of series.slice(1).entries()) expect(v).toBeGreaterThanOrEqual(series[i]);
    }
    expect(byStrokes[5]).toBeGreaterThan(byStrokes[0]);
    expect(bySpan[4]).toBeGreaterThan(bySpan[0]);
  });

  test("toggles closer than the minimum spacing give no edges", () => {
    expect(edges(400, 3, recorded)).toBe(0);
  });

  test("a pass shorter than the dead time gives no edges", () => {
    expect(edges(600, 10, diag(40))).toBe(0);
  });

  test("600 mm/s on 260 mm reaches the pass threshold, 6 segments at 600 mm/s do not", () => {
    expect(edges(600, 10, diag(260, 20))).toBeGreaterThanOrEqual(20);
    expect(edges(600, 6, diag(200))).toBeLessThan(20);
  });

  test("cartesian needs twice the segments for the corexy diagonal's estimate", () => {
    const alongX: Pattern = { start: [50, 150], end: [250, 150] };
    expect(edges(195, 6, alongX, 8, "cartesian")).toBe(edges(195, 3, diag(100)));
    expect(edges(195, 3, alongX, 8, "cartesian")).toBe(0);
  });

  test("unknown kinematics give no estimate", () => {
    expect(edges(195, 3, recorded, 8, "delta")).toBeNull();
  });
});

describe("probe edge hint", () => {
  const diag = (reach: number): Pattern => ({ start: [50, 50], end: [50 + reach, 50 + reach] });
  const hint = (speed: number, segments: number, reach: number, strokes = 8) =>
    probeEdgeHint({
      pattern: diag(reach),
      kinematics: "corexy",
      speedMmS: speed,
      segmentsBetweenToggles: segments,
      strokes,
      maxVelocity: 500,
    });

  test("40 or more is a neutral note", () => {
    const h = hint(195, 3, 100, 10)!;
    expect(h.tone).toBe("ok");
    expect(h.edges).toBe(49);
    expect(h.text).toContain("about 49 step responses");
    expect(h.text).toContain("at least 20");
  });

  test("the default probe parameters on a 100 mm diagonal need no caution", () => {
    const h = probeEdgeHint({
      pattern: diag(100),
      kinematics: "corexy",
      speedMmS: DEFAULT_PARAMETERS.probe.speed_mm_s,
      segmentsBetweenToggles: DEFAULT_PARAMETERS.probe.segments_between_toggles,
      strokes: DEFAULT_PARAMETERS.probe.strokes,
      maxVelocity: 500,
    })!;
    expect(h.tone).toBe("ok");
  });

  test("the suggested strokes are the fewest that clear the steady-estimate bar", () => {
    for (const [speed, segments, reach] of [
      [195, 3, 100],
      [195, 3, 150],
      [400, 6, 150],
      [195, 3, 70],
    ]) {
      const input = {
        pattern: diag(reach),
        kinematics: "corexy",
        speedMmS: speed,
        segmentsBetweenToggles: segments,
        maxVelocity: 500,
      };
      const strokes = suggestedProbeStrokes(input)!;
      expect(probeEdgeHint({ ...input, strokes })!.tone).toBe("ok");
      expect(probeEdgeHint({ ...input, strokes: strokes - 1 })?.tone).not.toBe("ok");
    }
  });

  test("no strokes are suggested when the toggles are too close or the pattern is too short", () => {
    const input = {
      pattern: diag(100),
      kinematics: "corexy",
      speedMmS: 195,
      segmentsBetweenToggles: 1,
      maxVelocity: 500,
    };
    expect(suggestedProbeStrokes(input)).toBeNull();
    expect(suggestedProbeStrokes({ ...input, segmentsBetweenToggles: 3, pattern: diag(5) })).toBeNull();
  });

  test("between 20 and 39 is a caution naming the strokes that reach 40", () => {
    const h = hint(400, 6, 150)!;
    expect(h.tone).toBe("caution");
    expect(h.text).toContain("about 26 step responses");
    expect(h.text).toContain("enough to run");
    expect(h.text).toContain("Use 13 strokes");
  });

  test("below 20 is a warning with the strokes and the pattern length that reach 40", () => {
    const h = hint(400, 6, 100)!;
    expect(h.tone).toBe("warning");
    expect(h.text).toContain("only about 12 step responses");
    expect(h.text).toContain("needs 20");
    expect(h.text).toContain("Use 25 strokes");
    expect(h.text).toContain("290 mm");
  });

  test("when 50 strokes cannot reach 40 only the pattern length is offered", () => {
    const h = hint(600, 10, 60)!;
    expect(h.tone).toBe("warning");
    expect(h.text).not.toContain("strokes (about");
    expect(h.text).toContain("Lengthen the pattern");
  });

  test("toggles in the low-yield spacing band ask for more segments", () => {
    const h = hint(600, 6, 200)!;
    expect(h.text).toContain("about 0 step responses");
    expect(h.text).toContain("Raise Moves between gain changes to 9 or more");
  });

  test("no hint when the spacing rule already blocks the run", () => {
    expect(hint(400, 3, 100)).toBeNull();
  });
});

describe("pattern mode", () => {
  const proposal: Pattern = { start: [60, 60], end: [240, 60] };
  const other: Pattern = { start: [50, 50], end: [250, 50] };

  test("without a proposal the pattern can only be custom", () => {
    expect(patternMode(undefined, null, null)).toBe("custom");
    expect(patternMode("default", null, other)).toBe("custom");
  });

  test("an explicit choice wins", () => {
    expect(patternMode("custom", proposal, proposal)).toBe("custom");
    expect(patternMode("default", proposal, other)).toBe("default");
  });

  test("with no choice, a remembered pattern that differs from the proposal is custom", () => {
    expect(patternMode(undefined, proposal, null)).toBe("default");
    expect(patternMode(undefined, proposal, proposal)).toBe("default");
    expect(patternMode(undefined, proposal, other)).toBe("custom");
  });
});

describe("ladderFieldErrors", () => {
  const input = (over: Partial<LadderInput> = {}): LadderInput => ({
    axis: "velocity_p",
    text: "100, 120",
    repeats: 2,
    floor: 0.05,
    marginal: 0.08,
    ...over,
  });
  const none = { values: null, runs: null, thresholds: null };

  test("a valid sweep has no errors", () => {
    expect(ladderFieldErrors(input(), 50_000, true)).toEqual(none);
  });

  test("an empty or malformed value list is an error on the values input, in plain words", () => {
    expect(ladderFieldErrors(input({ text: "  " }), 50_000, true).values).toBe(
      "Enter at least one value, separated by commas.",
    );
    expect(ladderFieldErrors(input({ text: "100, abc" }), 50_000, true).values).toBe('"abc" is not a number.');
  });

  test("too many values, or a value outside the allowed range, belong to the values input", () => {
    const many = Array.from({ length: 21 }, (_, i) => 100 + i).join(", ");
    expect(ladderFieldErrors(input({ text: many }), 50_000, true).values).toBe("Use at most 20 values.");
    expect(ladderFieldErrors(input({ text: "100, 70000" }), 50_000, true).values).toBe(
      "70000 is outside the allowed range of 0 to 32767 (raw).",
    );
    expect(ladderFieldErrors(input({ axis: "velocity_i_corner_hz", text: "5" }), null, true).values).toBe(
      "The PWM frequency is unknown, so values in Hz cannot be converted.",
    );
  });

  test("runs per value and the total number of runs are errors on the runs field", () => {
    expect(ladderFieldErrors(input({ repeats: 0 }), 50_000, true).runs).toBe(
      "Runs per value must be a whole number from 1 to 10.",
    );
    expect(ladderFieldErrors(input({ repeats: 11 }), 50_000, true).runs).toBe(
      "Runs per value must be a whole number from 1 to 10.",
    );
    const many = Array.from({ length: 10 }, (_, i) => 100 + i).join(", ");
    expect(ladderFieldErrors(input({ text: many, repeats: 7 }), 50_000, true).runs).toBe(
      "A sweep queues at most 60 runs; this one would queue 70.",
    );
  });

  test("non-numeric thresholds are errors on the thresholds", () => {
    expect(ladderFieldErrors(input({ floor: Number.NaN }), 50_000, true).thresholds).toBe(
      "Stop below damping and Mark as marginal below must be numbers.",
    );
  });

  test("the thresholds are not checked for sweeps that have no stop rule", () => {
    expect(ladderFieldErrors(input({ floor: Number.NaN }), 50_000, false).thresholds).toBeNull();
  });
});

describe("sweepOrder", () => {
  const entry = (id: string, value: number | null) => ({ id, run: { request: { ladder_value: value } } });

  test("sorts repeats next to each other by the sweep's own value order", () => {
    const queued = [
      entry("1", 300),
      entry("2", 100),
      entry("3", 200),
      entry("4", 300),
      entry("5", 100),
      entry("6", 200),
    ];
    expect(sweepOrder(queued, [300, 100, 200]).map((e) => e.id)).toEqual(["1", "4", "2", "5", "3", "6"]);
  });

  test("keeps the order within a value, and puts unknown values and non-sweep runs last in order", () => {
    const rows = [entry("a", 100), entry("b", 999), entry("c", 100), entry("d", null)];
    expect(sweepOrder(rows, [100]).map((e) => e.id)).toEqual(["a", "c", "b", "d"]);
  });

  test("without a sweep the order is untouched", () => {
    const rows = [entry("b", null), entry("a", null)];
    expect(sweepOrder(rows, null).map((e) => e.id)).toEqual(["b", "a"]);
  });
});

describe("run position and queue order", () => {
  const run = (id: string, state: string) => ({ id, state });

  test("the position counts finished runs, not where the id sorts", () => {
    const runs = [run("t-zzzz", "queued"), run("t-aaaa", "moving"), run("t-mmmm", "queued"), run("t-bbbb", "queued")];
    expect(runPosition(runs, "t-aaaa")).toEqual({ run: 1, of: 4 });
    const later = [run("t-zzzz", "done"), run("t-aaaa", "moving"), run("t-mmmm", "done"), run("t-bbbb", "queued")];
    expect(runPosition(later, "t-aaaa")).toEqual({ run: 3, of: 4 });
  });

  test("skipped and aborted runs do not count, and a single run has no position", () => {
    expect(runPosition([run("a", "moving"), run("b", "skipped"), run("c", "aborted")], "a")).toBeNull();
    expect(runPosition([run("a", "done"), run("b", "moving"), run("c", "skipped")], "b")).toEqual({ run: 2, of: 2 });
    expect(runPosition([run("a", "moving")], "z")).toBeNull();
  });

  test("the queue is listed in the engine's order, not the id order", () => {
    const runs = [run("a", "queued"), run("b", "queued"), run("c", "queued")];
    expect(queueInOrder(["c", "a", "b"], runs).map((r) => r.id)).toEqual(["c", "a", "b"]);
    expect(queueInOrder(["c", "gone"], runs).map((r) => r.id)).toEqual(["c"]);
  });
});

describe("runProgress", () => {
  const steps = (moving: number) => [
    { step: "preparing", unix_ms: 1_000 },
    { step: "moving", unix_ms: moving },
  ];

  test("the quick steps before the motion fill the first tenth", () => {
    expect(runProgress({ state: "preparing", steps: [], predictedS: 40, nowMs: 0 })?.fraction).toBe(0);
    expect(runProgress({ state: "recording", steps: [], predictedS: 40, nowMs: 0 })?.fraction).toBeCloseTo(0.075);
  });

  test("the motion fills 10% to 90% as the predicted time elapses, and says how long is left", () => {
    const at = (nowMs: number) => runProgress({ state: "moving", steps: steps(10_000), predictedS: 40, nowMs });
    expect(at(10_000)?.fraction).toBeCloseTo(0.1);
    expect(at(30_000)?.fraction).toBeCloseTo(0.5);
    expect(at(30_000)?.secondsLeft).toBe(20);
    expect(at(60_000)?.fraction).toBeLessThan(0.9);
    expect(at(60_000)?.secondsLeft).toBe(0);
  });

  test("without a prediction or a start time the motion holds at the start of its share", () => {
    expect(runProgress({ state: "moving", steps: steps(10_000), predictedS: null, nowMs: 50_000 })).toMatchObject({
      fraction: 0.1,
      secondsLeft: null,
    });
    expect(runProgress({ state: "moving", steps: [], predictedS: 40, nowMs: 50_000 })?.fraction).toBe(0.1);
  });

  test("draining, verifying and analyzing fill the last tenth, and unknown states have no bar", () => {
    expect(runProgress({ state: "draining", steps: [], predictedS: 40, nowMs: 0 })?.fraction).toBeCloseTo(0.9);
    expect(runProgress({ state: "analyzing", steps: [], predictedS: 40, nowMs: 0 })?.fraction).toBeCloseTo(0.9667, 3);
    expect(runProgress({ state: "killing", steps: [], predictedS: 40, nowMs: 0 })).toBeNull();
  });
});

describe("sweepRejectionNotes", () => {
  const base = { unit: "Hz", tested: "stepper_x", others: ["stepper_y"], skippedValues: [] as number[] };
  const rejected = (over: object = {}) => ({
    value: 25,
    state: "not_comparable",
    reason: null as string | null,
    failedChecks: [] as string[],
    ...over,
  });

  test("names the stepper that moved, and says what that means", () => {
    expect(sweepRejectionNotes({ ...base, rejected: [rejected({ failedChecks: ["channel_moved"] })] })).toEqual([
      "The run at 25 Hz was rejected: stepper_y moved while stepper_x was being tested, so the result can't be trusted.",
    ]);
  });

  test("falls back to another motor when the board has no single other stepper", () => {
    const notes = sweepRejectionNotes({
      ...base,
      others: [],
      rejected: [rejected({ failedChecks: ["channel_moved"] })],
    });
    expect(notes[0]).toContain("Another motor moved while stepper_x was being tested");
  });

  test("each other cause gets one plain sentence", () => {
    const note = (over: object) => sweepRejectionNotes({ ...base, rejected: [rejected(over)] })[0];
    expect(note({ failedChecks: ["baseline_contiguous"] })).toBe(
      "The run at 25 Hz was rejected: the gains changed during the run, so the result can't be trusted.",
    );
    expect(note({ failedChecks: ["no_drop_gaps"] })).toBe(
      "The run at 25 Hz was rejected: the recording lost data, so the result can't be trusted.",
    );
    expect(note({ failedChecks: ["edge_count"] })).toBe(
      "The run at 25 Hz was rejected: it recorded too few step responses to measure damping.",
    );
    expect(note({ state: "done" })).toBe("The run at 25 Hz was rejected: the recording shows no clear damping value.");
    expect(note({ state: "failed", reason: "klipper is not ready" })).toBe(
      "The run at 25 Hz failed: klipper is not ready.",
    );
    expect(note({})).toBe("The run at 25 Hz was rejected: its result can't be trusted.");
  });

  test("raw gains show no unit", () => {
    expect(
      sweepRejectionNotes({
        ...base,
        unit: "",
        rejected: [rejected({ value: 900, failedChecks: ["no_drop_gaps"] })],
      })[0],
    ).toContain("The run at 900 was rejected");
  });

  test("a stopped sweep names the values that did not run", () => {
    const one = sweepRejectionNotes({
      ...base,
      skippedValues: [50],
      rejected: [rejected({ failedChecks: ["channel_moved"] })],
    });
    expect(one.at(-1)).toBe("The sweep stopped after 25 Hz, so 50 Hz did not run.");
    const many = sweepRejectionNotes({
      ...base,
      skippedValues: [50, 75, 100],
      rejected: [rejected({ failedChecks: ["channel_moved"] })],
    });
    expect(many.at(-1)).toBe("The sweep stopped after 25 Hz, so 50, 75 and 100 Hz did not run.");
  });

  test("several rejected values each get a line", () => {
    const notes = sweepRejectionNotes({
      ...base,
      rejected: [rejected({ failedChecks: ["no_drop_gaps"] }), rejected({ value: 50, state: "done" })],
    });
    expect(notes).toHaveLength(2);
    expect(notes[1]).toContain("The run at 50 Hz");
  });
});

describe("patternIsolation", () => {
  const x = { name: "stepper_x", kinematics: "corexy" };
  const y = { name: "stepper_y", kinematics: "corexy" };
  const p = (sx: number, sy: number, ex: number, ey: number): Pattern => ({ start: [sx, sy], end: [ex, ey] });
  const limits = { x: [0, 301], y: [0, 308] } as { x: [number, number]; y: [number, number] };
  const check = (pattern: Pattern, stepper = x, other: string[] = ["stepper_y"]) =>
    patternIsolation({ pattern, stepper, others: other, limits });

  test("the proposed corexy diagonals isolate their stepper", () => {
    expect(check(p(100.5, 104, 200.5, 204)).ok).toBe(true);
    expect(check(p(100.5, 204, 200.5, 104), y, ["stepper_x"]).ok).toBe(true);
  });

  test("a path off the diagonal says which motor would move and by how much", () => {
    const r = check(p(22.7, 18.7, 262.2, 273.7));
    expect(r.ok).toBe(false);
    expect(r.message).toBe(
      "This path is not on the diagonal that moves only stepper_x. stepper_y would move 15.5 mm, and the run would be rejected.",
    );
  });

  test("only a millimetre of whole-number rounding is tolerated, leaving the engine's allowance for belt flex", () => {
    expect(check(p(100, 100, 200, 201)).ok).toBe(true);
    expect(check(p(100, 100, 200, 202)).ok).toBe(false);
    expect(check(p(100, 100, 200, 130)).ok).toBe(false);
  });

  test("a path nine millimetres off the diagonal is flagged, however long the stroke", () => {
    const r = check(p(30, 21, 270, 270));
    expect(r.ok).toBe(false);
    expect(r.message).toContain("stepper_y would move 9.0 mm");
  });

  test("with no other stepper on the board the message says the other motor", () => {
    expect(check(p(22.7, 18.7, 262.2, 273.7), x, []).message).toContain("The other motor would move 15.5 mm");
  });

  test("a cartesian stepper must run along its own axis", () => {
    const cx = { name: "stepper_x", kinematics: "cartesian" };
    expect(check(p(100, 150, 200, 150), cx).ok).toBe(true);
    const bad = check(p(100, 150, 200, 170), cx);
    expect(bad.ok).toBe(false);
    expect(bad.message).toBe(
      "This path is not along the X axis. stepper_y would move 20.0 mm, and the run would be rejected.",
    );
  });

  const length = (q: Pattern) => Math.hypot(q.end[0] - q.start[0], q.end[1] - q.start[1]);
  const mid = (q: Pattern) => [(q.start[0] + q.end[0]) / 2, (q.start[1] + q.end[1]) / 2];

  test("the fix rotates the path about its middle, keeps its length and lands on the diagonal", () => {
    const off = p(22.7, 18.7, 262.2, 273.7);
    const fixed = check(off).snapped;
    expect(check(fixed).ok).toBe(true);
    expect(Math.abs(length(fixed) - length(off))).toBeLessThan(2);
    expect(Math.abs(mid(fixed)[0] - mid(off)[0])).toBeLessThan(1);
    expect(Math.abs(mid(fixed)[1] - mid(off)[1])).toBeLessThan(1);
    expect(fixed.end[0] - fixed.start[0]).toBe(fixed.end[1] - fixed.start[1]);
    expect(patternProblem(fixed, limits)).toBeNull();
  });

  test("the fix is exactly 45 degrees on a corexy and exactly on the axis on a cartesian", () => {
    const dist = (q: Pattern) => [q.end[0] - q.start[0], q.end[1] - q.start[1]];
    const ex = check(p(22.7, 18.7, 262.2, 273.7)).snapped;
    expect(Math.abs(dist(ex)[0])).toBe(Math.abs(dist(ex)[1]));
    expect(dist(ex)[0]).toBe(dist(ex)[1]);
    const ey = check(p(100, 200, 200, 90), y, ["stepper_x"]).snapped;
    expect(dist(ey)[0]).toBe(-dist(ey)[1]);
    const cx = { name: "stepper_x", kinematics: "cartesian" };
    const ax = check(p(100, 150, 200, 170), cx).snapped;
    expect(dist(ax)[1]).toBe(0);
    const cy = { name: "stepper_y", kinematics: "cartesian" };
    const ay = check(p(100, 150, 120, 250), cy, ["stepper_x"]).snapped;
    expect(dist(ay)[0]).toBe(0);
  });

  test("the fix gives whole-millimetre coordinates", () => {
    const fixed = check(p(22.7, 18.7, 262.2, 273.7)).snapped;
    for (const v of [...fixed.start, ...fixed.end]) expect(Number.isInteger(v)).toBe(true);
  });

  test("the fix shifts the path back inside the bed margin when rotating would leave it", () => {
    const off = p(200, 100, 280, 240);
    const fixed = check(off).snapped;
    expect(check(fixed).ok).toBe(true);
    expect(patternProblem(fixed, limits)).toBeNull();
    expect(Math.abs(length(fixed) - length(off))).toBeLessThan(2);
  });

  test("the fix shortens the path only when it cannot fit at its length", () => {
    const off = p(10, 298, 291, 10);
    const fixed = check(off).snapped;
    expect(check(fixed).ok).toBe(true);
    expect(patternProblem(fixed, limits)).toBeNull();
    expect(length(fixed)).toBeLessThan(length(off));
  });

  test("the fix follows the direction the path already goes", () => {
    const back = check(p(250, 250, 50, 40)).snapped;
    expect(back.end[0]).toBeLessThan(back.start[0]);
    expect(back.end[1]).toBeLessThan(back.start[1]);
    const anti = check(p(100, 200, 200, 90), y, ["stepper_x"]).snapped;
    expect(anti.end[0]).toBeGreaterThan(anti.start[0]);
    expect(anti.end[1]).toBeLessThan(anti.start[1]);
  });
});

describe("skipReasonText", () => {
  test("a low-damping stop reads as one plain sentence", () => {
    expect(skipReasonText("ladder stopped: median zeta 0.038 below the floor 0.05 at velocity_i_corner_hz 25")).toBe(
      "The sweep stopped after 25 Hz: its damping, 0.038, was below the stop limit of 0.05.",
    );
    expect(skipReasonText("ladder stopped: median zeta 0.02 below the floor 0.05 at velocity_p 900")).toBe(
      "The sweep stopped after 900: its damping, 0.02, was below the stop limit of 0.05.",
    );
  });

  test("an unmeasurable damping stop says so", () => {
    expect(skipReasonText("ladder stopped: damping unassessable at velocity_i_corner_hz 25")).toBe(
      "The sweep stopped after 25 Hz because its damping could not be measured.",
    );
  });

  test("any other reason is shown as it is", () => {
    expect(skipReasonText("ladder floor reached")).toBe("ladder floor reached");
    expect(skipReasonText("run.json: truncated")).toBe("run.json: truncated");
  });
});

describe("runStatusLabel", () => {
  test("a standstill run holds position, so its motion stage is measuring", () => {
    expect(runStatusLabel("moving", "standstill")).toBe("measuring");
  });
  test("tracking and probe runs, and every other stage, keep their stage name", () => {
    expect(runStatusLabel("moving", "tracking")).toBe("moving");
    expect(runStatusLabel("moving", "probe")).toBe("moving");
    expect(runStatusLabel("moving", undefined)).toBe("moving");
    expect(runStatusLabel("verifying", "standstill")).toBe("verifying");
    expect(runStatusLabel("writing_gains", "standstill")).toBe("writing gains");
  });
});
