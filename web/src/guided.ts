import type {
  Halt,
  LadderAssessment,
  Pause,
  InadmissibleReason,
  RungNote,
  Tune,
  TuneAxis,
  TuneStep,
  TuneState,
  UnscoredReason,
} from "./api";
import { expectedProbeEdges, PROBE_MIN_EDGES, probeToggleSpacing, type ProbeEdgeInput } from "./tune";

export interface ProbeSpeedChoice {
  speed: number;
  warning: string | null;
}

/** The fastest speed whose toggle spacing is valid and whose edge estimate reaches the minimum. */
export function defaultProbeSpeed(
  speeds: number[],
  context: Omit<ProbeEdgeInput, "speedMmS">,
): ProbeSpeedChoice {
  const fastestFirst = [...speeds].filter(Number.isFinite).sort((a, b) => b - a);
  if (fastestFirst.length === 0) return { speed: 0, warning: "No tracking speed to derive the probe speed from" };
  const spacingOk = (speedMmS: number) =>
    probeToggleSpacing(
      context.pattern,
      context.kinematics,
      speedMmS,
      context.segmentsBetweenToggles,
      context.maxVelocity,
    )?.ok === true;
  const edges = (speedMmS: number) => expectedProbeEdges({ ...context, speedMmS });

  const valid = fastestFirst.filter(spacingOk);
  const enough = valid.find((s) => (edges(s) ?? 0) >= PROBE_MIN_EDGES);
  if (enough !== undefined) return { speed: enough, warning: null };
  if (valid.length === 0) {
    return {
      speed: fastestFirst[0],
      warning:
        "No speed gives gain toggles far enough apart for the probe fit: raise the segments between toggles",
    };
  }
  return {
    speed: valid[0],
    warning:
      `No speed reaches ${PROBE_MIN_EDGES} expected probe edges ` +
      `(${edges(valid[0]) ?? 0} at ${valid[0]} mm/s): add strokes or lengthen the pattern`,
  };
}

export type RecommendationKind = "measured" | "interpolated";

export interface RungView {
  index: number;
  shown: number;
  raw: number;
  verdict: string;
  skipped: string | null;
  marginal: boolean;
  inadmissible: boolean;
  unscored: boolean;
  score: number | null;
  zeta: number | null;
  p99BySpeed: (number | null)[];
  worstTorqueSd: number | null;
  recommended: RecommendationKind | null;
  repeatsDone: number;
}

const UNSCORED_TEXT: Record<UnscoredReason, string> = {
  TrackingMissing: "tracking missing",
  StandstillMissing: "standstill missing",
  DampingWaived: "damping waived",
};

const INADMISSIBLE_TEXT: Record<InadmissibleReason, string> = {
  DampingBelowFloor: "damping below the floor",
  StandstillHigh: "standstill high",
};

function pauseText(reason: Pause): string {
  return reason === "DampingUnassessable" ? "damping unassessable" : "standstill baseline unassessable";
}

function haltText(halt: Halt): string {
  if (typeof halt === "string") return pauseText(halt);
  return `damping median ${halt.BelowFloor.median.toFixed(3)} below the floor`;
}

export type RecommendationView =
  | { kind: "measured"; rung: number }
  | { kind: "interpolated"; shown: number; between: [number, number] }
  | { kind: "none" };

export interface StepSummary {
  axis: TuneAxis;
  notes: string[];
  recommendation: RecommendationView | null;
  extend: "Below" | "Above" | null;
  halt: string | null;
  pause: string | null;
  failed: string | null;
}

export function noteText(note: RungNote): string {
  if ("OutOfRange" in note) return `${note.OutOfRange} left out: outside the writable range`;
  if ("RoundedToZero" in note) return `${note.RoundedToZero} left out: rounds to a raw value of zero`;
  return `${note.Merged.shown} merged into ${note.Merged.into}: same raw value`;
}

function usableAssessment(step: TuneStep): LadderAssessment | null {
  const a = step.assessment;
  if (a === null) return null;
  return a.verdicts.length === step.rungs.length && a.medians.length === step.rungs.length ? a : null;
}

/** Notes, recommendation and halt of one step, as the operator reads them. */
export function stepSummary(tune: Tune, step: number): StepSummary {
  const s = tune.steps[step];
  const choice = usableAssessment(s) === null ? undefined : s.recommendation?.choice;
  let recommendation: RecommendationView | null = null;
  if (choice !== undefined) {
    if ("Measured" in choice) recommendation = { kind: "measured", rung: choice.Measured.rung };
    else if ("Interpolated" in choice) {
      recommendation = {
        kind: "interpolated",
        shown: choice.Interpolated.shown,
        between: choice.Interpolated.between,
      };
    } else recommendation = { kind: "none" };
  }
  const halt = usableAssessment(s)?.halt ?? null;
  const pause: [number, Pause] | null =
    s.paused !== null ? [s.paused.rung, s.paused.reason] : (usableAssessment(s)?.pause ?? null);
  return {
    axis: s.axis,
    notes: s.notes.map(noteText),
    recommendation,
    extend: usableAssessment(s) === null ? null : (s.recommendation?.extend ?? null),
    halt: halt === null ? null : `Assessment stopped at rung ${halt[0] + 1}: ${haltText(halt[1])}`,
    pause: pause === null ? null : `Paused at rung ${pause[0] + 1}: ${pauseText(pause[1])}`,
    failed: s.failed,
  };
}

/** One row per rung of a step: gain, verdict with its reason, scores and per-speed p99. */
function notAssessedText(index: number, pause: [number, Pause] | null, halt: [number, Halt] | null): string {
  if (pause !== null) {
    if (index === pause[0]) return `Not assessed: ${pauseText(pause[1])}`;
    if (index > pause[0]) return `Not assessed: paused at rung ${pause[0] + 1}`;
  }
  if (halt !== null) {
    if (index > halt[0]) return `Not assessed: assessment stopped at rung ${halt[0] + 1}`;
    if (index === halt[0]) return `Not assessed: ${haltText(halt[1])}`;
  }
  return "Not assessed";
}

export function rungView(tune: Tune, step: number): RungView[] {
  const s = tune.steps[step];
  const assessment = usableAssessment(s);
  const choice = assessment === null ? undefined : s.recommendation?.choice;
  const halt = assessment?.halt ?? null;
  const pause = assessment?.pause ?? null;
  return s.rungs.map((rung, index) => {
    const verdict = assessment?.verdicts[index];
    const medians = assessment?.medians[index];
    const flagged =
      verdict !== undefined &&
      (verdict.inadmissible !== null || verdict.unscored !== null || verdict.marginal || verdict.score !== null);
    const repeatsDone = rung.repeat_values.length;

    let text: string;
    if (rung.skipped !== null) text = `Skipped: ${rung.skipped}`;
    else if (rung.waived !== null) text = `Waived: ${pauseText(rung.waived.reason)}`;
    else if (verdict?.inadmissible != null) text = `Inadmissible: ${INADMISSIBLE_TEXT[verdict.inadmissible]}`;
    else if (verdict?.unscored != null) text = `Unscored: ${UNSCORED_TEXT[verdict.unscored]}`;
    else if (verdict?.marginal) text = "Marginal damping";
    else if (verdict?.score != null) text = "Admissible";
    else if (assessment !== null && !flagged && (repeatsDone >= s.repeats || pause !== null || halt !== null)) {
      text = notAssessedText(index, pause, halt);
    } else if (repeatsDone >= s.repeats) text = "Measured, awaiting verdict";
    else if (repeatsDone > 0) text = `${repeatsDone} of ${s.repeats} repeats`;
    else text = "Pending";

    if (s.standstill_gate_off !== null) text = `${text} (Standstill gate off)`;

    let recommended: RecommendationKind | null = null;
    if (choice !== undefined && "Measured" in choice && choice.Measured.rung === index) recommended = "measured";
    if (choice !== undefined && "Interpolated" in choice && choice.Interpolated.between.includes(index)) {
      recommended = "interpolated";
    }

    return {
      index,
      shown: rung.shown,
      raw: rung.raw,
      verdict: text,
      skipped: rung.skipped,
      marginal: verdict?.marginal ?? false,
      inadmissible: verdict?.inadmissible != null,
      unscored: verdict?.unscored != null,
      score: verdict?.score ?? null,
      zeta: medians?.zeta ?? null,
      p99BySpeed: medians?.p99_by_speed ?? [],
      worstTorqueSd: medians?.worst_torque_sd ?? null,
      recommended,
      repeatsDone,
    };
  });
}

/** True while the seed rung's drift re-run is moving the machine after the last rung. */
export function driftCheckRunning(tune: Tune, step: number): boolean {
  const s = tune.steps[step];
  return tune.state === "Running" && s.drift_rung !== null && s.drift.length === 0 && s.ended_unix_ms === null;
}

export function isLocked(state: TuneState, lock: string | null): boolean {
  return lock !== null || state === "RestorePending" || typeof state === "object";
}

/** The step the operator is currently working on, or null before the first one. */
export function currentStep(tune: Tune): number | null {
  return tune.steps.length === 0 ? null : tune.steps.length - 1;
}

/** Whether the operator may pick a value now. */
export function canChoose(tune: Tune): boolean {
  return tune.state === "AwaitingChoice";
}

export interface Actions {
  prepare: boolean;
  start: boolean;
  choose: boolean;
  decide: boolean;
  extension: boolean;
  abandon: boolean;
  restore: boolean;
  acknowledge: boolean;
}

/** Which controls the guided view offers for the tune's state and its stepper lock. */
export function availableActions(tune: Tune, lock: string | null): Actions {
  const off: Actions = {
    prepare: false,
    start: false,
    choose: false,
    decide: false,
    extension: false,
    abandon: false,
    restore: false,
    acknowledge: false,
  };
  if (isLocked(tune.state, lock)) return { ...off, restore: true, acknowledge: true };
  switch (tune.state) {
    case "Ready":
      return { ...off, prepare: true, abandon: true };
    case "Announced":
      return { ...off, start: true, abandon: true };
    case "Running":
      return { ...off, abandon: true };
    case "AwaitingChoice":
      return { ...off, choose: true, extension: true, abandon: true };
    case "Paused":
      return { ...off, decide: true, abandon: true };
    default:
      return off;
  }
}

/** Mean recorded run time of earlier tunes times `runCount`, in seconds; null without any. */
export function expectedDurationSeconds(tunes: Tune[], runCount: number): number | null {
  let total = 0;
  let runs = 0;
  for (const tune of tunes) {
    for (const step of tune.steps) {
      for (const rung of [...step.rungs, ...(step.drift_rung === null ? [] : [step.drift_rung])]) {
        for (const t of rung.run_times) {
          if (t.ended_unix_ms >= t.started_unix_ms) {
            total += t.ended_unix_ms - t.started_unix_ms;
            runs += 1;
          }
        }
      }
    }
  }
  return runs === 0 ? null : (total / runs / 1000) * runCount;
}

const AXIS_LABEL: Record<TuneAxis, string> = {
  VelocityP: "Velocity P",
  VelocityICornerHz: "Velocity I corner (Hz)",
  PositionP: "Position P",
};

export const axisLabel = (axis: TuneAxis): string => AXIS_LABEL[axis];

export function stateText(state: TuneState): string {
  if (typeof state === "object") return `Restore failed: ${state.RestoreFailed}`;
  return {
    Ready: "Ready for the next step",
    Announced: "Step announced",
    Running: "Running",
    AwaitingChoice: "Awaiting your choice",
    Paused: "Paused: waiting for your decision",
    Done: "Done",
    Abandoned: "Abandoned",
    RestorePending: "Awaiting its restore",
  }[state];
}

export function durationText(seconds: number | null): string {
  if (seconds === null) return "unknown";
  const minutes = Math.round(seconds / 60);
  return minutes < 1 ? `${Math.round(seconds)} s` : `about ${minutes} min`;
}

const AXIS_ORDER: TuneAxis[] = ["VelocityP", "VelocityICornerHz", "PositionP"];

/** The axis the next prepared step tunes; null once all three have a step. */
export function nextAxis(tune: Tune): TuneAxis | null {
  return AXIS_ORDER[tune.steps.length] ?? null;
}

export interface Announcement {
  stepper: string;
  axis: TuneAxis;
  firstGain: number;
  lastGain: number;
  rungs: number;
  repeats: number;
  runCount: number;
}

/** What the announced step will do, read from the stored step so it survives a reload. */
export function announcementOf(tune: Tune): Announcement | null {
  const step = tune.steps[tune.steps.length - 1];
  if (step === undefined || step.rungs.length === 0) return null;
  const perRepeat = tune.setup.speeds_mm_s.length + 2;
  const seedInLadder = step.rungs.some((r) => r.raw === step.seed_raw);
  const computed = step.rungs.length * step.repeats * perRepeat + (seedInLadder ? perRepeat : 0);
  return {
    stepper: tune.setup.stepper,
    axis: step.axis,
    firstGain: step.rungs[0].shown,
    lastGain: step.rungs[step.rungs.length - 1].shown,
    rungs: step.rungs.length,
    repeats: step.repeats,
    runCount: step.run_count ?? computed,
  };
}

/** uPlot columns for the per-speed p99 curve: rung gains, then one column per tracking speed. */
export function curveData(rows: RungView[], speedCount: number): [number[], ...(number | null)[][]] {
  return [
    rows.map((r) => r.shown),
    ...Array.from({ length: speedCount }, (_, s) => rows.map((r) => r.p99BySpeed[s] ?? null)),
  ];
}

/** The gain the recommendation marks on the curve, with how it was found. */
export function recommendedGain(tune: Tune, step: number): { gain: number; kind: RecommendationKind } | null {
  const rec = stepSummary(tune, step).recommendation;
  if (rec === null || rec.kind === "none") return null;
  if (rec.kind === "interpolated") return { gain: rec.shown, kind: "interpolated" };
  const rung = tune.steps[step].rungs[rec.rung];
  return rung === undefined ? null : { gain: rung.shown, kind: "measured" };
}
