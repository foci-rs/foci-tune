import { ApiRequestError } from "./api";
import type {
  ChannelMetrics,
  Filters,
  LadderAxis,
  LadderRequestBody,
  LiveChannel,
  Pattern,
  Run,
  RunKind,
  RunRegime,
  RunId,
  RunRequestBody,
  RunSetup,
  SessionId,
  SessionView,
  StepperView,
} from "./api";
import { velocityIRaw } from "./units";

export type RunType = RunSetup["type"];
export type Setup = { stepper: string; pattern: Pattern } & RunSetup;
export type RunBody = RunRequestBody;
export type OuterGains = RunRequestBody["gains"];

export const PID_GAIN_MAX_RAW = 0x7fff;
export const PATTERN_MARGIN_MM = 10;
const MAX_LADDER_VALUES = 20;
const MAX_LADDER_REPEATS = 10;
const MAX_LADDER_RUNS = 60;
const POSITION_UNITS_PER_REV = 65536;

/** A stroke along X through the bed centre, up to 100 mm and shortened to stay inside the margins. */
export function fallbackPattern(limits: Limits): Pattern {
  const cx = (limits.x[0] + limits.x[1]) / 2;
  const cy = (limits.y[0] + limits.y[1]) / 2;
  return { start: clampPoint([cx - 50, cy], limits), end: clampPoint([cx + 50, cy], limits) };
}

export const DEFAULT_PARAMETERS: { [K in RunType]: Extract<RunSetup, { type: K }>["parameters"] } = {
  tracking: { speeds_mm_s: [100, 200, 400], strokes: 8 },
  probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 9 },
  standstill: { rests: 8, dwell_s: 3 },
};

function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((v, k) => sameValue(v, b[k]));
  }
  if (typeof a === "object" && typeof b === "object" && a !== null && b !== null) {
    const ka = Object.keys(a);
    const kb = Object.keys(b);
    return (
      ka.length === kb.length &&
      ka.every((k) => sameValue((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]))
    );
  }
  return false;
}

/** True when `b` cannot continue the session set up as `a`. */
export function nextSession(a: Setup, b: Setup): boolean {
  return (
    a.stepper !== b.stepper ||
    a.type !== b.type ||
    !sameValue(a.pattern, b.pattern) ||
    !sameValue(a.parameters, b.parameters)
  );
}

export interface SessionLink {
  id: SessionId;
  setup: Setup;
}

/** The session a single run joins: the linked one while the setup is unchanged, else none. */
export function sessionForRun(link: SessionLink | null, setup: Setup): SessionId | null {
  return link !== null && !nextSession(link.setup, setup) ? link.id : null;
}

export function setupOfSession(view: SessionView): Setup | null {
  const s = view.session;
  return s === null ? null : setupFromKind(s.stepper, s.pattern, s.kind);
}

/**
 * The session to show when the page has none: the session of the run the engine executes,
 * else the session holding a queued run. An idle engine restores nothing, so an old session is
 * never resumed silently.
 */
export function pickRestoreSession(
  running: SessionId | null,
  queue: RunId[],
  sessions: SessionView[],
): SessionId | null {
  if (running !== null) return running;
  return sessions.find((v) => v.runs.some((r) => queue.includes(r.id)))?.id ?? null;
}

export interface RestoreBasis {
  link: SessionId | null;
  form: string;
}

/** A restore applies only if neither the link nor the form changed while it was fetched. */
export function restoreStillApplies(started: RestoreBasis, now: RestoreBasis): boolean {
  return started.link === now.link && started.form === now.form;
}

/** True when the error says the linked session is gone: not found, or refused as unknown. */
export function isUnknownSession(e: unknown): boolean {
  return (
    e instanceof ApiRequestError && (e.status === 404 || (e.status === 409 && e.message.startsWith("unknown session")))
  );
}

export function runTypeOfKind(kind: RunKind): RunType {
  return "Tracking" in kind ? "tracking" : "Probe" in kind ? "probe" : "standstill";
}

export function setupFromKind(stepper: string, pattern: Pattern, kind: RunKind): Setup {
  if ("Tracking" in kind) return { stepper, pattern, type: "tracking", parameters: kind.Tracking };
  if ("Probe" in kind) return { stepper, pattern, type: "probe", parameters: kind.Probe };
  return { stepper, pattern, type: "standstill", parameters: kind.Standstill };
}

export type Parsed<T> = { ok: true; values: T } | { ok: false; error: string };

export function parseNumberList(text: string, what: string): Parsed<number[]> {
  const parts = text
    .split(",")
    .map((p) => p.trim())
    .filter((p) => p !== "");
  if (parts.length === 0) return { ok: false, error: `${what} needs at least one value` };
  const values: number[] = [];
  for (const part of parts) {
    const v = Number(part);
    if (!Number.isFinite(v)) return { ok: false, error: `${part} is not a number` };
    values.push(v);
  }
  return { ok: true, values };
}

export function parseLadderList(text: string): Parsed<number[]> {
  return parseNumberList(text, "the ladder");
}

export type GainRow = OuterGains & Filters;

export function declaredMetrics(run: Run): ChannelMetrics | null {
  const channels = run.metrics?.channels ?? [];
  return channels.find((c) => c.declared) ?? channels.find((c) => c.channel === run.declared_channel) ?? null;
}

export interface Headline {
  label: string;
  value: number | null;
  unit: string;
  note?: string;
}

/**
 * The headline metric of the declared channel: p99 error at the highest tracking speed, the
 * velocity fit's zeta for a probe, the worst rest torque sd for standstill.
 */
export function headlineMetric(run: Run, type: RunType): Headline {
  const m = declaredMetrics(run);
  switch (type) {
    case "tracking": {
      const top = [...(m?.tracking ?? [])].sort((a, b) => b.speed_mm_s - a.speed_mm_s)[0];
      return {
        label: top === undefined ? "p99" : `p99 at ${top.speed_mm_s} mm/s`,
        value: top?.p99_um ?? null,
        unit: "μm",
      };
    }
    case "probe": {
      const fit = m?.probe?.velocity;
      if (fit?.status === "failed") {
        return { label: "velocity zeta", value: null, unit: "", note: fit.reason };
      }
      return { label: "velocity zeta", value: fit?.zeta ?? null, unit: "" };
    }
    case "standstill": {
      const rate = m?.standstill?.worst_large_transitions_per_s;
      const metric = { label: "worst torque sd", value: m?.standstill?.worst_torque_sd ?? null, unit: "raw" };
      return typeof rate === "number" ? { ...metric, note: `large steps ${formatNumber(rate, 1)}/s` } : metric;
    }
  }
}

export function formatNumber(value: number | null | undefined, digits = 2): string {
  return value === null || value === undefined || !Number.isFinite(value) ? "n/a" : value.toFixed(digits);
}

export function axisToRaw(axis: LadderAxis, shown: number, pwmHz: number): number {
  return axis === "velocity_i_corner_hz" ? velocityIRaw(shown, pwmHz) : Math.round(shown);
}

export interface LadderInput {
  axis: LadderAxis;
  text: string;
  repeats: number;
  floor: number;
  marginal: number;
}

export type Built<T> = { ok: true; body: T } | { ok: false; error: string };

export interface LadderFieldErrors {
  values: string | null;
  runs: string | null;
  thresholds: string | null;
}

export function ladderFieldErrors(input: LadderInput, pwmHz: number | null, probe: boolean): LadderFieldErrors {
  const errors: LadderFieldErrors = { values: null, runs: null, thresholds: null };
  const hz = input.axis === "velocity_i_corner_hz";
  const parts = input.text
    .split(",")
    .map((p) => p.trim())
    .filter((p) => p !== "");
  const values: number[] = [];
  if (parts.length === 0) {
    errors.values = "Enter at least one value, separated by commas.";
  } else {
    for (const part of parts) {
      const v = Number(part);
      if (!Number.isFinite(v)) {
        errors.values = `"${part}" is not a number.`;
        break;
      }
      values.push(v);
    }
  }
  if (errors.values === null && values.length > MAX_LADDER_VALUES) {
    errors.values = `Use at most ${MAX_LADDER_VALUES} values.`;
  }
  if (errors.values === null && hz && (pwmHz === null || pwmHz <= 0)) {
    errors.values = "The PWM frequency is unknown, so values in Hz cannot be converted.";
  }
  if (errors.values === null) {
    for (const v of values) {
      const raw = axisToRaw(input.axis, v, pwmHz ?? 0);
      if (raw < 0 || raw > PID_GAIN_MAX_RAW) {
        errors.values = hz
          ? `${v} Hz is outside the allowed range: it converts to raw ${raw}, and the limit is ${PID_GAIN_MAX_RAW}.`
          : `${v} is outside the allowed range of 0 to ${PID_GAIN_MAX_RAW} (raw).`;
        break;
      }
    }
  }
  if (!Number.isInteger(input.repeats) || input.repeats < 1 || input.repeats > MAX_LADDER_REPEATS) {
    errors.runs = `Runs per value must be a whole number from 1 to ${MAX_LADDER_REPEATS}.`;
  } else if (values.length * input.repeats > MAX_LADDER_RUNS) {
    errors.runs = `A sweep queues at most ${MAX_LADDER_RUNS} runs; this one would queue ${values.length * input.repeats}.`;
  }
  if (probe && (!Number.isFinite(input.floor) || !Number.isFinite(input.marginal))) {
    errors.thresholds = "Stop below damping and Mark as marginal below must be numbers.";
  }
  return errors;
}

/** The ladder request, checked the way the engine checks it so a refusal shows before posting. */
export function buildLadderRequest(base: RunBody, input: LadderInput, pwmHz: number | null): Built<LadderRequestBody> {
  const { session: _session, ...fixed } = base;
  const parsed = parseLadderList(input.text);
  if (!parsed.ok) return parsed;
  const values = parsed.values;
  if (!Number.isInteger(input.repeats) || input.repeats < 1) {
    return { ok: false, error: "repeats must be a whole number of at least 1" };
  }
  if (values.length > MAX_LADDER_VALUES) {
    return { ok: false, error: `values must be at most ${MAX_LADDER_VALUES}` };
  }
  if (input.repeats > MAX_LADDER_REPEATS) {
    return { ok: false, error: `repeats must be at most ${MAX_LADDER_REPEATS}` };
  }
  const runs = values.length * input.repeats;
  if (runs > MAX_LADDER_RUNS) {
    return {
      ok: false,
      error: `a ladder queues at most ${MAX_LADDER_RUNS} runs; this one would queue ${runs}`,
    };
  }
  if (input.axis === "velocity_i_corner_hz" && (pwmHz === null || pwmHz <= 0)) {
    return { ok: false, error: "the PWM frequency is unknown, so a corner cannot be converted" };
  }
  for (const v of values) {
    const raw = axisToRaw(input.axis, v, pwmHz ?? 0);
    if (raw < 0 || raw > PID_GAIN_MAX_RAW) {
      return { ok: false, error: `${v} converts to raw ${raw}, outside 0..=${PID_GAIN_MAX_RAW}` };
    }
  }
  if (!Number.isFinite(input.floor) || !Number.isFinite(input.marginal)) {
    return { ok: false, error: "floor and marginal must be numbers" };
  }
  return {
    ok: true,
    body: {
      ...fixed,
      axis: input.axis,
      values,
      repeats: input.repeats,
      floor: input.floor,
      marginal: input.marginal,
    },
  };
}

/**
 * Default rungs from the current gains; the current value itself and out-of-range rungs are
 * left out.
 */
export function defaultLadderValues(axis: LadderAxis, gains: OuterGains, pwmHz: number): number[] {
  const steps = [0, 1, 2, 3, 4, 5];
  const shown =
    axis === "velocity_p"
      ? [-2, -1, 1, 2, 3, 4].map((k) => gains.velocity_p + 100 * k)
      : axis === "position_p"
        ? steps.map((k) => gains.position_p - 20 + 20 * k)
        : [25, 50, 75, 100];
  if (axis === "velocity_i_corner_hz" && pwmHz <= 0) return [];
  return shown.filter((v) => {
    const raw = axisToRaw(axis, v, pwmHz);
    return raw >= 0 && raw <= PID_GAIN_MAX_RAW;
  });
}

export interface LiveRegime {
  clock_freq: number;
  motor_rotation_distance_mm: number;
}

/** The constants live samples need: the batch's clock when known, else the run's record. */
export function liveRegime(
  batchClock: number,
  run: RunRegime | null,
  stepper: Pick<StepperView, "motor_rotation_distance_mm"> | null,
): { ok: true; regime: LiveRegime } | { ok: false; error: string } {
  const clock = batchClock > 0 ? batchClock : (run?.clock_freq ?? 0);
  const distance = run?.motor_rotation_distance_mm ?? stepper?.motor_rotation_distance_mm ?? 0;
  if (clock <= 0) return { ok: false, error: "the board's clock frequency is unknown" };
  if (!(distance > 0)) return { ok: false, error: "the motor rotation distance is unknown" };
  return { ok: true, regime: { clock_freq: clock, motor_rotation_distance_mm: distance } };
}

export interface LiveTrace {
  firstTick: number;
  lastTick: number;
  lastPosition: number;
  t_s: number[];
  position_error_um: number[];
  velocity_mm_s: (number | null)[];
  torque: number[];
}

/** Appends one batch of raw samples, keeping the last `windowS` seconds. */
export function appendLive(trace: LiveTrace | null, ch: LiveChannel, regime: LiveRegime, windowS: number): LiveTrace {
  const umPerUnit = (regime.motor_rotation_distance_mm / POSITION_UNITS_PER_REV) * 1000;
  const next: LiveTrace = trace
    ? {
        ...trace,
        t_s: [...trace.t_s],
        position_error_um: [...trace.position_error_um],
        velocity_mm_s: [...trace.velocity_mm_s],
        torque: [...trace.torque],
      }
    : {
        firstTick: ch.ticks[0] ?? 0,
        lastTick: Number.NaN,
        lastPosition: 0,
        t_s: [],
        position_error_um: [],
        velocity_mm_s: [],
        torque: [],
      };
  ch.ticks.forEach((tick, k) => {
    const t = (tick - next.firstTick) / regime.clock_freq;
    const dt = (tick - next.lastTick) / regime.clock_freq;
    const position = ch.tmc_position[k];
    const velocity = Number.isFinite(dt) && dt > 0 ? ((position - next.lastPosition) * umPerUnit) / 1000 / dt : null;
    next.t_s.push(t);
    next.position_error_um.push(ch.position_error[k] * umPerUnit);
    next.velocity_mm_s.push(velocity);
    next.torque.push(ch.torque[k]);
    next.lastTick = tick;
    next.lastPosition = position;
  });
  const latest = next.t_s[next.t_s.length - 1] ?? 0;
  const drop = next.t_s.findIndex((t) => t >= latest - windowS);
  if (drop > 0) {
    next.t_s.splice(0, drop);
    next.position_error_um.splice(0, drop);
    next.velocity_mm_s.splice(0, drop);
    next.torque.splice(0, drop);
  }
  return next;
}

export type Column = (number | null)[];
export type AlignedColumns = [number[], ...(number | null | undefined)[][]];

/**
 * Joins tables `[x, ...ys]` onto one sorted x axis. A null y stays null so the plot breaks
 * the line there; a sample a table does not have is undefined so the plot spans it.
 */
export function alignSeries(tables: [Column, ...Column[]][]): AlignedColumns {
  const xs = new Set<number>();
  for (const [x] of tables) for (const v of x) if (v !== null && Number.isFinite(v)) xs.add(v);
  const axis = [...xs].sort((a, b) => a - b);
  const index = new Map(axis.map((v, k) => [v, k]));
  const out: AlignedColumns = [axis];
  for (const [x, ...ys] of tables) {
    for (const y of ys) {
      const col: (number | null | undefined)[] = new Array<number | null | undefined>(axis.length).fill(undefined);
      x.forEach((xv, k) => {
        if (xv === null) return;
        const at = index.get(xv);
        if (at === undefined) return;
        const v = y[k];
        col[at] = v === null || v === undefined || !Number.isFinite(v) ? null : v;
      });
      out.push(col);
    }
  }
  return out;
}

export function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const s = [...values].sort((a, b) => a - b);
  const mid = Math.floor(s.length / 2);
  return s.length % 2 === 1 ? s[mid] : (s[mid - 1] + s[mid]) / 2;
}

export interface LadderEntry {
  value: number | null;
  metric: number | null;
  /** Finished `done` with a metric; anything else cannot be assessed. */
  assessable: boolean;
}

export interface LadderMedian {
  value: number;
  median: number | null;
  unassessable: boolean;
}

/**
 * The median per value over its first `repeats` runs in run order, as the ladder stop rule
 * takes it: none until every repeat has finished, and none, marked unassessable, when any of
 * them was not `done` or has no metric.
 */
export function ladderMedians(entries: LadderEntry[], repeats: number): LadderMedian[] {
  const groups = new Map<number, LadderEntry[]>();
  for (const e of entries) {
    if (e.value === null) continue;
    groups.set(e.value, [...(groups.get(e.value) ?? []), e]);
  }
  return [...groups.keys()]
    .sort((a, b) => a - b)
    .map((value) => {
      const first = (groups.get(value) ?? []).slice(0, repeats);
      const unassessable = first.some((e) => !e.assessable || e.metric === null);
      const complete = first.length >= repeats && !unassessable;
      return {
        value,
        median: complete ? median(first.map((e) => e.metric as number)) : null,
        unassessable,
      };
    });
}

/** `[values, repeat 1, ..., repeat n, median]` for the ladder plot. */
export function ladderPlotData(entries: LadderEntry[], repeats: number): [number[], ...(number | null)[][]] {
  const medians = ladderMedians(entries, repeats);
  const values = medians.map((m) => m.value);
  const byValue = new Map<number, (number | null)[]>();
  for (const e of entries) {
    if (e.value === null) continue;
    byValue.set(e.value, [...(byValue.get(e.value) ?? []), e.metric]);
  }
  const reps = Array.from({ length: repeats }, (_, r) => values.map((v) => byValue.get(v)?.[r] ?? null));
  return [values, ...reps, medians.map((m) => m.median)];
}

/** Keeps each checked run's colour slot; a newly checked run takes the lowest free slot. */
export function assignColors(prev: Map<string, number>, checked: string[]): Map<string, number> {
  const next = new Map<string, number>();
  for (const id of checked) {
    const slot = prev.get(id);
    if (slot !== undefined) next.set(id, slot);
  }
  for (const id of checked) {
    if (next.has(id)) continue;
    const used = new Set(next.values());
    let slot = 0;
    while (used.has(slot)) slot += 1;
    next.set(id, slot);
  }
  return next;
}

export interface Limits {
  x: [number, number];
  y: [number, number];
}

/** Widest X and Y limits over the steppers, as the service derives the bed. */
export function bedLimits(steppers: StepperView[]): Limits | null {
  const span = (axis: string): [number, number] | null =>
    steppers
      .filter((s) => s.axis === axis)
      .reduce<[number, number] | null>(
        (acc, s) =>
          acc === null
            ? [s.position_min, s.position_max]
            : [Math.min(acc[0], s.position_min), Math.max(acc[1], s.position_max)],
        null,
      );
  const x = span("x");
  const y = span("y");
  return x === null || y === null ? null : { x, y };
}

export function clampPoint(p: [number, number], limits: Limits): [number, number] {
  const clamp = (v: number, [lo, hi]: [number, number]) =>
    Math.min(hi - PATTERN_MARGIN_MM, Math.max(lo + PATTERN_MARGIN_MM, v));
  return [clamp(p[0], limits.x), clamp(p[1], limits.y)];
}

/** Why the service would refuse the pattern, naming the broken limit; null when it is valid. */
export function patternProblem(p: Pattern, limits: Limits): string | null {
  const checks: [string, number, [number, number]][] = [
    ["start X", p.start[0], limits.x],
    ["start Y", p.start[1], limits.y],
    ["end X", p.end[0], limits.x],
    ["end Y", p.end[1], limits.y],
  ];
  for (const [name, v, [lo, hi]] of checks) {
    const min = lo + PATTERN_MARGIN_MM;
    const max = hi - PATTERN_MARGIN_MM;
    if (!Number.isFinite(v) || v < min || v > max) {
      return `${name} ${v} is outside ${min}..${max} mm (bed limits minus the ${PATTERN_MARGIN_MM} mm margin)`;
    }
  }
  if (Math.hypot(p.end[0] - p.start[0], p.end[1] - p.start[1]) < 1) {
    return "the pattern is shorter than 1 mm";
  }
  return null;
}

export function patternInside(p: Pattern, limits: Limits): boolean {
  return [p.start, p.end].every((pt) => {
    const c = clampPoint(pt, limits);
    return c[0] === pt[0] && c[1] === pt[1];
  });
}

/** Mirrors `MIN_TOGGLE_SPACING_S` in the stimulus composer. */
export const MIN_TOGGLE_SPACING_S = 0.1;
const SEGMENT_MM = 5;
const SPACING_TOLERANCE_S = 1e-9;

function pathToStepperRatio(kinematics: string): number | null {
  return kinematics === "corexy" ? Math.SQRT1_2 : kinematics === "cartesian" ? 1 : null;
}

/** Stepper travel of one stroke segment as the composer cuts it, in mm; null for other kinematics. */
export function segmentTravelMm(p: Pattern, kinematics: string): number | null {
  const ratio = pathToStepperRatio(kinematics);
  if (ratio === null) return null;
  const [dx, dy] = [p.end[0] - p.start[0], p.end[1] - p.start[1]];
  const n = Math.max(1, Math.ceil(Math.max(Math.abs(dx), Math.abs(dy)) / SEGMENT_MM));
  return Math.hypot(dx, dy) / n / ratio;
}

export interface ToggleSpacing {
  spacingS: number;
  /** Stepper speed after the `max_velocity` clamp. */
  speedMmS: number;
  minSegments: number;
  /** The fewest segments at which the gain changes are far enough apart to give clean step responses. */
  cleanSegments: number;
  ok: boolean;
}

/** The probe's predicted time between gain toggles; null when the inputs leave it undefined. */
export function probeToggleSpacing(
  p: Pattern,
  kinematics: string,
  speedMmS: number,
  segments: number,
  maxVelocity: number,
): ToggleSpacing | null {
  const ratio = pathToStepperRatio(kinematics);
  const travel = segmentTravelMm(p, kinematics);
  if (ratio === null || travel === null || !(travel > 0) || !(speedMmS > 0) || !(segments >= 1)) {
    return null;
  }
  const speed = maxVelocity > 0 ? Math.min(speedMmS, maxVelocity / ratio) : speedMmS;
  const spacingS = (travel * segments) / speed;
  const needed = (MIN_TOGGLE_SPACING_S * speed) / travel - SPACING_TOLERANCE_S;
  return {
    spacingS,
    speedMmS: speed,
    minSegments: Math.max(1, Math.ceil(needed)),
    cleanSegments: Math.max(
      1,
      Math.ceil((Math.max(PROBE_FULL_YIELD_SPACING_S, MIN_TOGGLE_SPACING_S) * speed) / travel - ESTIMATE_TOLERANCE),
    ),
    ok: spacingS + SPACING_TOLERANCE_S >= MIN_TOGGLE_SPACING_S,
  };
}

/** Why the probe's edge detector would find no edges at this spacing; null when it is far enough. */
export function toggleSpacingProblem(s: ToggleSpacing): string | null {
  if (s.ok) return null;
  return (
    `At ${s.speedMmS.toFixed(0)} mm/s gain changes are ${(s.spacingS * 1000).toFixed(0)} ms apart; the fit needs ` +
    `at least ${(MIN_TOGGLE_SPACING_S * 1000).toFixed(0)} ms and about ${(PROBE_FULL_YIELD_SPACING_S * 1000).toFixed(0)} ms ` +
    `gives clean step responses: use ${s.cleanSegments} or more moves between gain changes`
  );
}

/** Mirrors `MIN_PROBE_EDGES` in the run verifier. */
export const PROBE_MIN_EDGES = 20;
/** Edge count from which the ensemble damping estimate is steady enough to compare runs. */
export const PROBE_STEADY_EDGES = 40;
/** Mirrors `MAX_STROKES` in the stimulus composer. */
const PROBE_MAX_STROKES = 50;
// The gain toggles run when Klipper parses them, not in step with the motion, so they lose
// their spacing below about 150 ms.
/** Time of each pass in which no clean edge lands: the reversal and the toggle lag. */
const PROBE_DEAD_S = 0.26;
/** Toggle spacing at and below which no edge was recorded clean. */
const PROBE_NO_YIELD_SPACING_S = 0.105;
/** Toggle spacing from which every toggle away from the reversals gave a clean edge. */
const PROBE_FULL_YIELD_SPACING_S = 0.15;
const ESTIMATE_TOLERANCE = 1e-9;

export interface ProbeEdgeInput {
  pattern: Pattern;
  kinematics: string;
  /** Requested stepper speed; the `max_velocity` clamp is applied here. */
  speedMmS: number;
  segmentsBetweenToggles: number;
  strokes: number;
  maxVelocity: number;
}

interface EdgeModel {
  perStroke: number;
  spacing: ToggleSpacing;
  ratio: number;
  yieldFraction: number;
}

function edgeModel(input: ProbeEdgeInput): EdgeModel | null {
  const { pattern, kinematics, speedMmS, segmentsBetweenToggles, maxVelocity } = input;
  const ratio = pathToStepperRatio(kinematics);
  const spacing = probeToggleSpacing(pattern, kinematics, speedMmS, segmentsBetweenToggles, maxVelocity);
  if (ratio === null || spacing === null) return null;
  const lengthMm = Math.hypot(pattern.end[0] - pattern.start[0], pattern.end[1] - pattern.start[1]);
  const passS = lengthMm / ratio / spacing.speedMmS;
  const band = PROBE_FULL_YIELD_SPACING_S - PROBE_NO_YIELD_SPACING_S;
  const yieldFraction = Math.min(1, Math.max(0, (spacing.spacingS - PROBE_NO_YIELD_SPACING_S) / band));
  const perStroke = spacing.ok ? (Math.max(0, passS - PROBE_DEAD_S) / spacing.spacingS) * yieldFraction : 0;
  return { perStroke, spacing, ratio, yieldFraction };
}

const floorEdges = (v: number) => Math.max(0, Math.floor(v + ESTIMATE_TOLERANCE));

/** Conservative estimate of the probe edges a run will locate; null for other kinematics. */
export function expectedProbeEdges(input: ProbeEdgeInput): number | null {
  const m = edgeModel(input);
  return m === null ? null : floorEdges(m.perStroke * Math.max(0, input.strokes));
}

export interface ProbeEdgeHint {
  tone: "ok" | "caution" | "warning";
  edges: number;
  text: string;
}

function edgeSuggestion(m: EdgeModel, input: ProbeEdgeInput): string {
  if (m.yieldFraction === 0) {
    const travel = segmentTravelMm(input.pattern, input.kinematics) as number;
    const segments = Math.ceil((PROBE_FULL_YIELD_SPACING_S * m.spacing.speedMmS) / travel - ESTIMATE_TOLERANCE);
    const ms = (m.spacing.spacingS * 1000).toFixed(0);
    return `Gain changes ${ms} ms apart rarely give clean step responses. Raise Moves between gain changes to ${segments} or more.`;
  }
  const passS = (PROBE_STEADY_EDGES * m.spacing.spacingS) / (input.strokes * m.yieldFraction) + PROBE_DEAD_S;
  const lengthMm = Math.ceil((passS * m.spacing.speedMmS * m.ratio) / 10) * 10;
  const lengthen = `lengthen the pattern to about ${lengthMm} mm`;
  if (m.perStroke > 0) {
    const strokes = Math.ceil(PROBE_STEADY_EDGES / m.perStroke - ESTIMATE_TOLERANCE);
    if (strokes <= PROBE_MAX_STROKES) {
      return `Use ${strokes} strokes (about ${floorEdges(m.perStroke * strokes)}), or ${lengthen}.`;
    }
  }
  return `${lengthen[0].toUpperCase()}${lengthen.slice(1)}.`;
}

/** The fewest strokes whose expected edges reach the steady-estimate bar; null when none do. */
export function suggestedProbeStrokes(input: Omit<ProbeEdgeInput, "strokes">): number | null {
  const m = edgeModel({ ...input, strokes: 1 });
  if (m === null || !m.spacing.ok || m.perStroke <= 0) return null;
  const strokes = Math.ceil(PROBE_STEADY_EDGES / m.perStroke - ESTIMATE_TOLERANCE);
  return strokes <= PROBE_MAX_STROKES ? strokes : null;
}

/** Null when there is no estimate to give. */
export function probeEdgeHint(input: ProbeEdgeInput): ProbeEdgeHint | null {
  const m = edgeModel(input);
  if (m === null || !m.spacing.ok) return null;
  const n = floorEdges(m.perStroke * Math.max(0, input.strokes));
  if (n >= PROBE_STEADY_EDGES) {
    return {
      tone: "ok",
      edges: n,
      text:
        `This setup should record about ${n} step responses. The test needs at least ${PROBE_MIN_EDGES}; ` +
        `${PROBE_STEADY_EDGES} or more gives a steadier damping result.`,
    };
  }
  const suggestion = edgeSuggestion(m, input);
  if (n >= PROBE_MIN_EDGES) {
    return {
      tone: "caution",
      edges: n,
      text:
        `This setup should record about ${n} step responses. That is enough to run, but ` +
        `${PROBE_STEADY_EDGES} or more gives a steadier damping result. ${suggestion}`,
    };
  }
  return {
    tone: "warning",
    edges: n,
    text:
      `This setup should record only about ${n} step responses, and the test needs ${PROBE_MIN_EDGES}. ` +
      `Its result probably can't be compared with other runs. ${suggestion}`,
  };
}

/** Which run of its session the current one is: finished runs before it plus one, out of all that will run. */
export function runPosition(
  runs: { id: string; state: string }[],
  currentId: string,
): { run: number; of: number } | null {
  const counted = runs.filter((r) => r.state !== "skipped" && r.state !== "aborted");
  if (counted.length < 2 || !counted.some((r) => r.id === currentId)) return null;
  return { run: counted.filter((r) => TERMINAL_STATES.has(r.state)).length + 1, of: counted.length };
}

/** The queued runs in the order the engine will take them; ids that are not in the list are dropped. */
export function queueInOrder<T extends { id: string }>(queue: string[], runs: T[]): T[] {
  return queue.flatMap((id) => runs.filter((r) => r.id === id));
}

/**
 * Runs in the order of the sweep: by the value's place in the sweep's own list, repeats of a
 * value side by side in the order they ran. Runs without a known value keep their order, last.
 */
export function sweepOrder<T extends { run: { request: { ladder_value: number | null } } | null }>(
  entries: T[],
  values: number[] | null,
): T[] {
  if (values === null) return entries;
  const place = (e: T) => {
    const v = e.run?.request.ladder_value ?? null;
    const i = v === null ? -1 : values.indexOf(v);
    return i < 0 ? values.length : i;
  };
  return entries
    .map((e, order) => ({ e, order }))
    .sort((a, b) => place(a.e) - place(b.e) || a.order - b.order)
    .map((x) => x.e);
}

export const RUN_STEPS = [
  "preparing",
  "writing_gains",
  "baseline",
  "recording",
  "moving",
  "draining",
  "verifying",
  "analyzing",
] as const;

export interface RunProgress {
  /** 0 to 1 along the whole run. */
  fraction: number;
  /** One-based place in [`RUN_STEPS`]. */
  step: number;
  /** Predicted seconds left of the motion; null outside it or without a prediction. */
  secondsLeft: number | null;
}

const MOVING_AT = RUN_STEPS.indexOf("moving");
const BEFORE_SHARE = 0.1;
const AFTER_SHARE = 0.1;

/**
 * How far along a run is. The quick stages before the motion fill the first tenth and the last
 * three the final tenth; the motion fills the rest as the predicted script time elapses, held
 * short of the end until the run leaves it.
 */
export function runProgress(input: {
  state: string;
  steps: { step: string; unix_ms: number }[];
  predictedS: number | null;
  nowMs: number;
}): RunProgress | null {
  const index = (RUN_STEPS as readonly string[]).indexOf(input.state);
  if (index < 0) return null;
  const step = index + 1;
  if (index < MOVING_AT) return { fraction: (index * BEFORE_SHARE) / MOVING_AT, step, secondsLeft: null };
  if (index > MOVING_AT) {
    const after = RUN_STEPS.length - MOVING_AT - 1;
    return { fraction: 1 - AFTER_SHARE + ((index - MOVING_AT - 1) * AFTER_SHARE) / after, step, secondsLeft: null };
  }
  const started = [...input.steps].reverse().find((s) => s.step === "moving")?.unix_ms;
  if (started === undefined || input.predictedS === null || input.predictedS <= 0) {
    return { fraction: BEFORE_SHARE, step, secondsLeft: null };
  }
  const elapsedS = Math.max(0, (input.nowMs - started) / 1000);
  const share = Math.min(0.99, elapsedS / input.predictedS);
  return {
    fraction: BEFORE_SHARE + (1 - BEFORE_SHARE - AFTER_SHARE) * share,
    step,
    secondsLeft: Math.max(0, Math.round(input.predictedS - elapsedS)),
  };
}

export interface RejectedRun {
  value: number;
  state: string;
  reason: string | null;
  failedChecks: string[];
}

function valueList(values: number[], unit: string): string {
  const shown = values.map(String);
  const list = shown.length === 1 ? shown[0] : `${shown.slice(0, -1).join(", ")} and ${shown[shown.length - 1]}`;
  return unit === "" ? list : `${list} ${unit}`;
}

/** Why a run was rejected or failed, as a clause without a subject; the caller adds the run. */
export function rejectionReason(run: RejectedRun, tested: string, others: string[]): string {
  const moved = others.length === 1 ? others[0] : "Another motor";
  if (run.state === "failed") return `${run.reason ?? "no reason was recorded"}.`;
  if (run.failedChecks.includes("channel_moved")) {
    return `${moved} moved while ${tested} was being tested, so the result can't be trusted.`;
  }
  if (run.failedChecks.includes("baseline_contiguous")) {
    return "the gains changed during the run, so the result can't be trusted.";
  }
  if (run.failedChecks.includes("no_drop_gaps")) return "the recording lost data, so the result can't be trusted.";
  if (run.failedChecks.includes("edge_count")) return "it recorded too few step responses to measure damping.";
  if (run.state === "done") return "the recording shows no clear damping value.";
  return "its result can't be trusted.";
}

function rejectionCause(run: RejectedRun, tested: string, others: string[]): string {
  const reason = rejectionReason(run, tested, others);
  return run.state === "failed" ? `failed: ${reason}` : `was rejected: ${reason}`;
}

/**
 * `rejected` is in sweep order, so its first value is where the sweep stopped.
 */
export function sweepRejectionNotes(input: {
  rejected: RejectedRun[];
  skippedValues: number[];
  unit: string;
  tested: string;
  others: string[];
}): string[] {
  const lines = input.rejected.map((run) => {
    return `The run at ${valueList([run.value], input.unit)} ${rejectionCause(run, input.tested, input.others)}`;
  });
  if (input.skippedValues.length > 0 && input.rejected.length > 0) {
    const first = valueList([input.rejected[0].value], input.unit);
    lines.push(`The sweep stopped after ${first}, so ${valueList(input.skippedValues, input.unit)} did not run.`);
  }
  return lines;
}

const PATH_ROUNDING_TOLERANCE_MM = 1;

export interface Isolation {
  ok: boolean;
  /** What is wrong, in plain words; empty when the path is fine. */
  message: string;
  /** The path turned about its middle onto the direction that moves only the tested motor. */
  snapped: Pattern;
  snapLabel: string;
}

/**
 * Whether a stroke pattern moves only the tested stepper, the way the engine's "other motor
 * moved" check needs: on a corexy the stepper's own diagonal, on a cartesian its own axis, to
 * within a millimetre. Other kinematics and steppers are not checked.
 */
export function patternIsolation(input: {
  pattern: Pattern;
  stepper: { name: string; kinematics: string };
  others: string[];
  limits: Limits | null;
}): Isolation {
  const { pattern, stepper, others, limits } = input;
  const unchecked: Isolation = { ok: true, message: "", snapped: pattern, snapLabel: "" };
  const axis = stepper.name === "stepper_x" ? "x" : stepper.name === "stepper_y" ? "y" : null;
  const corexy = stepper.kinematics === "corexy";
  if (axis === null || (!corexy && stepper.kinematics !== "cartesian")) return unchecked;
  const dx = pattern.end[0] - pattern.start[0];
  const dy = pattern.end[1] - pattern.start[1];
  const [, other] = corexy
    ? axis === "x"
      ? [Math.abs(dx + dy), Math.abs(dx - dy)]
      : [Math.abs(dx - dy), Math.abs(dx + dy)]
    : axis === "x"
      ? [Math.abs(dx), Math.abs(dy)]
      : [Math.abs(dy), Math.abs(dx)];
  if (other <= PATH_ROUNDING_TOLERANCE_MM) return unchecked;

  const sign = (v: number) => (v < 0 ? -1 : 1);
  const length = Math.hypot(dx, dy);
  const dir: [number, number] = corexy
    ? [sign(axis === "x" ? dx + dy : dx - dy), 0]
    : axis === "x"
      ? [sign(dx), 0]
      : [0, sign(dy)];
  if (corexy) dir[1] = axis === "x" ? dir[0] : -dir[0];
  const centre: [number, number] = [
    Math.round((pattern.start[0] + pattern.end[0]) / 2),
    Math.round((pattern.start[1] + pattern.end[1]) / 2),
  ];
  const span = (c: number, half: number, step: number): [number, number] => [c - half * step, c + half * step];
  const place = (half: number): Pattern | null => {
    let [cx, cy] = centre;
    if (limits !== null) {
      const shift = (c: number, step: number, [lo, hi]: [number, number]): number | null => {
        const min = lo + PATTERN_MARGIN_MM - Math.min(...span(c, half, step));
        const max = hi - PATTERN_MARGIN_MM - Math.max(...span(c, half, step));
        return min > max ? null : Math.min(Math.max(0, min), max);
      };
      const sx = shift(cx, dir[0], limits.x);
      const sy = shift(cy, dir[1], limits.y);
      if (sx === null || sy === null) return null;
      cx += sx;
      cy += sy;
    }
    return {
      start: [cx - dir[0] * half, cy - dir[1] * half],
      end: [cx + dir[0] * half, cy + dir[1] * half],
    };
  };
  const reach = corexy ? Math.SQRT2 : 1;
  let half = Math.max(1, Math.round(length / (2 * reach)));
  let snapped = place(half);
  while (snapped === null && half > 1) {
    half -= 1;
    snapped = place(half);
  }
  snapped ??= { start: pattern.start, end: pattern.end };
  const who = others.length === 1 ? others[0] : "The other motor";
  const where = corexy ? `the diagonal that moves only ${stepper.name}` : `the ${axis.toUpperCase()} axis`;
  const prefix = corexy ? "on" : "along";
  return {
    ok: false,
    message: `This path is not ${prefix} ${where}. ${who} would move ${other.toFixed(1)} mm, and the run would be rejected.`,
    snapped,
    snapLabel: "Fix the path",
  };
}

const AXIS_UNIT: Record<string, string> = { velocity_i_corner_hz: "Hz" };

/**
 * Unrecognised reasons are shown as they are.
 */
export function skipReasonText(reason: string): string {
  const low = /^ladder stopped: median zeta ([\d.]+) below the floor ([\d.]+) at (\w+) ([\d.]+)$/.exec(reason);
  if (low !== null) {
    const unit = AXIS_UNIT[low[3]];
    return `The sweep stopped after ${low[4]}${unit === undefined ? "" : ` ${unit}`}: its damping, ${low[1]}, was below the stop limit of ${low[2]}.`;
  }
  const unmeasured = /^ladder stopped: damping unassessable at (\w+) ([\d.]+)$/.exec(reason);
  if (unmeasured !== null) {
    const unit = AXIS_UNIT[unmeasured[1]];
    return `The sweep stopped after ${unmeasured[2]}${unit === undefined ? "" : ` ${unit}`} because its damping could not be measured.`;
  }
  return reason;
}

export type PatternMode = "default" | "custom";

const samePattern = (a: Pattern, b: Pattern) =>
  a.start[0] === b.start[0] && a.start[1] === b.start[1] && a.end[0] === b.end[0] && a.end[1] === b.end[1];

/**
 * Whether a stepper's pattern follows the proposal or is typed: always custom without a
 * proposal, otherwise the operator's choice, otherwise custom only when a remembered pattern
 * differs from the proposal.
 */
export function patternMode(
  chosen: PatternMode | undefined,
  proposal: Pattern | null,
  remembered: Pattern | null,
): PatternMode {
  if (proposal === null) return "custom";
  if (chosen !== undefined) return chosen;
  return remembered !== null && !samePattern(remembered, proposal) ? "custom" : "default";
}

export interface PatternStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const patternKey = (stepper: string) => `foci-tune.pattern.${stepper}`;

function isPoint(v: unknown): v is [number, number] {
  return Array.isArray(v) && v.length === 2 && v.every((n) => typeof n === "number" && Number.isFinite(n));
}

export function loadPattern(storage: PatternStorage | null, stepper: string): Pattern | null {
  try {
    const text = storage?.getItem(patternKey(stepper));
    if (text === null || text === undefined) return null;
    const v = JSON.parse(text) as { start?: unknown; end?: unknown };
    return isPoint(v.start) && isPoint(v.end) ? { start: v.start, end: v.end } : null;
  } catch {
    return null;
  }
}

export function savePattern(storage: PatternStorage | null, stepper: string, pattern: Pattern): void {
  try {
    storage?.setItem(patternKey(stepper), JSON.stringify(pattern));
  } catch {}
}

export function browserStorage(): PatternStorage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export const RECORDING_STATES: ReadonlySet<string> = new Set(["baseline", "recording", "moving", "draining"]);

export const TERMINAL_STATES: ReadonlySet<string> = new Set([
  "done",
  "not_comparable",
  "failed",
  "aborted",
  "skipped",
  "kill_unconfirmed",
  "killed",
]);

export function stateLabel(state: string): string {
  return state.replace(/_/g, " ");
}

/** A run's stage as the operator reads it: a standstill run holds position, so its motion stage is measuring. */
export function runStatusLabel(state: string, type: RunType | undefined): string {
  return state === "moving" && type === "standstill" ? "measuring" : stateLabel(state);
}
