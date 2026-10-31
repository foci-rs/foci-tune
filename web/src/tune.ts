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
  RunRequestBody,
  RunSetup,
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

export const DEFAULT_PARAMETERS: { [K in RunType]: Extract<RunSetup, { type: K }>["parameters"] } = {
  tracking: { speeds_mm_s: [50, 195, 400], strokes: 8 },
  probe: { speed_mm_s: 195, high_gain: 1.3, segments_between_toggles: 3, strokes: 8 },
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

/** For each row, the keys whose value differs from the row before it. */
export function highlightChanges<T extends { [K in keyof T]: number }>(rows: T[]): Set<keyof T>[] {
  return rows.map((row, k) => {
    const changed = new Set<keyof T>();
    if (k === 0) return changed;
    const prev = rows[k - 1];
    for (const key of Object.keys(row) as (keyof T)[]) {
      if (prev[key] !== row[key]) changed.add(key);
    }
    return changed;
  });
}

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

/** The run type's headline metric for the declared channel, as the spec names it. */
export function headlineMetric(run: Run, type: RunType): Headline {
  const m = declaredMetrics(run);
  switch (type) {
    case "tracking": {
      const top = [...(m?.tracking ?? [])].sort((a, b) => b.speed_mm_s - a.speed_mm_s)[0];
      return {
        label: top === undefined ? "p99" : `p99 at ${top.speed_mm_s} mm/s`,
        value: top?.p99_um ?? null,
        unit: "um",
      };
    }
    case "probe": {
      const fit = m?.probe?.velocity;
      if (fit?.status === "failed") {
        return { label: "velocity zeta", value: null, unit: "", note: fit.reason };
      }
      return { label: "velocity zeta", value: fit?.zeta ?? null, unit: "" };
    }
    case "standstill":
      return { label: "worst torque sd", value: m?.standstill?.worst_torque_sd ?? null, unit: "raw" };
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

/** The ladder request, checked the way the engine checks it so a refusal shows before posting. */
export function buildLadderRequest(
  base: RunBody,
  input: LadderInput,
  pwmHz: number | null,
): Built<LadderRequestBody> {
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
      ...base,
      axis: input.axis,
      values,
      repeats: input.repeats,
      floor: input.floor,
      marginal: input.marginal,
    },
  };
}

/** The spec's default rungs from the current gains, every out-of-range rung omitted. */
export function defaultLadderValues(axis: LadderAxis, gains: OuterGains, pwmHz: number): number[] {
  const steps = [0, 1, 2, 3, 4, 5];
  const shown =
    axis === "velocity_p"
      ? steps.map((k) => gains.velocity_p + 100 * k)
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
export function appendLive(
  trace: LiveTrace | null,
  ch: LiveChannel,
  regime: LiveRegime,
  windowS: number,
): LiveTrace {
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
    const velocity =
      Number.isFinite(dt) && dt > 0 ? ((position - next.lastPosition) * umPerUnit) / 1000 / dt : null;
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
      const col: (number | null | undefined)[] = new Array<number | null | undefined>(axis.length).fill(
        undefined,
      );
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

/**
 * `[values, repeat 1, ..., repeat n, median]`. The median is null for a value with fewer
 * metrics than repeats, matching the stop rule that never takes it over fewer repeats.
 */
export function ladderPlotData(
  entries: { value: number | null; metric: number | null }[],
  repeats: number,
): [number[], ...(number | null)[][]] {
  const groups = new Map<number, (number | null)[]>();
  for (const e of entries) {
    if (e.value === null) continue;
    const g = groups.get(e.value) ?? [];
    g.push(e.metric);
    groups.set(e.value, g);
  }
  const values = [...groups.keys()].sort((a, b) => a - b);
  const reps = Array.from({ length: repeats }, (_, r) => values.map((v) => groups.get(v)?.[r] ?? null));
  const medians = values.map((v) => {
    const metrics = (groups.get(v) ?? []).filter((m): m is number => m !== null);
    return metrics.length >= repeats ? median(metrics) : null;
  });
  return [values, ...reps, medians];
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
        (acc, s) => (acc === null ? [s.position_min, s.position_max] : [Math.min(acc[0], s.position_min), Math.max(acc[1], s.position_max)]),
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

export function patternInside(p: Pattern, limits: Limits): boolean {
  return [p.start, p.end].every((pt) => {
    const c = clampPoint(pt, limits);
    return c[0] === pt[0] && c[1] === pt[1];
  });
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
  } catch {
    // Remembering the pattern is a convenience; a refused write changes nothing else.
  }
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
