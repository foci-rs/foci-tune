import type {
  Halt,
  LadderAssessment,
  Pause,
  InadmissibleReason,
  RungNote,
  Tune,
  TuneDrift,
  TuneGates,
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
export function defaultProbeSpeed(speeds: number[], context: Omit<ProbeEdgeInput, "speedMmS">): ProbeSpeedChoice {
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
      warning: "No speed leaves enough time between gain changes for the probe fit. Raise Moves between gain changes.",
    };
  }
  return {
    speed: valid[0],
    warning:
      `No speed gives the probe enough step responses: about ${edges(valid[0]) ?? 0} at ` +
      `${valid[0]} mm/s, and it needs ${PROBE_MIN_EDGES}. Add strokes or lengthen the pattern.`,
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
  decidingSpeed: number | null;
  gap: number | null;
  tolerance: number | null;
  best: boolean;
}

const UNSCORED_TEXT: Record<UnscoredReason, string> = {
  TrackingMissing: "no tracking result",
  StandstillMissing: "no standstill result",
  DampingWaived: "damping check skipped",
};

const INADMISSIBLE_TEXT: Record<InadmissibleReason, string> = {
  DampingBelowFloor: "damping below the stop limit",
  StandstillHigh: "too much noise at standstill",
};

function pauseText(reason: Pause): string {
  return reason === "DampingUnassessable"
    ? "the damping could not be measured"
    : "the noise at standstill could not be measured";
}

function haltText(halt: Halt): string {
  if (typeof halt === "string") return pauseText(halt);
  return `the damping, ${halt.BelowFloor.median.toFixed(3)}, was below the stop limit`;
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
  comparison: string | null;
  driftLines: string[];
}

const gainAt = (step: TuneStep, index: number): string => step.rungs[index]?.shown.toFixed(2) ?? `${index + 1}`;

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
  const assessment = usableAssessment(s);
  const choice = assessment === null ? undefined : s.recommendation?.choice;
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
  const halt = assessment?.halt ?? null;
  const pause: [number, Pause] | null =
    s.paused !== null ? [s.paused.rung, s.paused.reason] : (assessment?.pause ?? null);
  return {
    axis: s.axis,
    notes: s.notes.map(noteText),
    recommendation,
    extend: assessment === null ? null : (s.recommendation?.extend ?? null),
    halt: halt === null ? null : `The sweep stopped at gain ${gainAt(s, halt[0])}: ${haltText(halt[1])}.`,
    pause: pause === null ? null : `Paused at gain ${gainAt(s, pause[0])}: ${pauseText(pause[1])}.`,
    failed: s.failed,
    comparison: assessment === null ? null : comparisonText(assessment, s.repeats),
    driftLines: driftLines(s.drift),
  };
}

const um = (value: number): string => `${value.toFixed(1)} μm`;

function rungList(indices: number[]): string {
  const shown = indices.map((i) => i + 1);
  return shown.length < 2 ? `${shown[0]}` : `${shown.slice(0, -1).join(", ")} and ${shown[shown.length - 1]}`;
}

/** How the recommended rung was chosen over its closest competitor; null without a best rung. */
export function comparisonText(a: LadderAssessment, repeats: number): string | null {
  if (a.best === null) return null;
  if (a.tie.length > 1) {
    const widest = a.tie
      .filter((i) => i !== a.best)
      .map((i) => a.verdicts[i])
      .reduce((max, v) => ((v.gap ?? 0) > (max.gap ?? 0) ? v : max));
    const tolerance = widest.tolerance === null ? "" : ` within ${um(widest.tolerance)}`;
    return `Tied: rungs ${rungList(a.tie)}, widest gap ${um(widest.gap ?? 0)}${tolerance}, so the lower gain was preferred.`;
  }
  if (a.runner_up === null) return "This is the only scored rung: there was nothing to compare.";
  const runnerUp = a.verdicts[a.runner_up];
  const name = `rung ${a.runner_up + 1}`;
  if (repeats < 2) {
    return `The runner-up, ${name}, is ${um(runnerUp.gap ?? 0)} behind; with one repeat ties cannot be detected.`;
  }
  const tolerance = runnerUp.tolerance === null ? "" : `, beyond the ${um(runnerUp.tolerance)} tolerance`;
  return `The runner-up, ${name}, is ${um(runnerUp.gap ?? 0)} behind${tolerance}.`;
}

/** One line per speed whose re-run drifted or could not be assessed. */
export function driftLines(drift: TuneDrift[]): string[] {
  return drift.flatMap((d) => {
    if (d.verdict === "Drifted") {
      return [
        `${d.metric}: ${d.first ?? "n/a"} to ${d.rerun ?? "n/a"}, drifted by more than the ladder's resolution at this speed; the pick may depend on run order`,
      ];
    }
    if (d.verdict === "Unassessed") return [`${d.metric}: drift could not be assessed`];
    return [];
  });
}

/** One row per rung of a step: gain, verdict with its reason, scores and per-speed p99. */
function notAssessedText(
  s: TuneStep,
  index: number,
  pause: [number, Pause] | null,
  halt: [number, Halt] | null,
): string {
  if (pause !== null) {
    if (index === pause[0]) return `Not checked: ${pauseText(pause[1])}`;
    if (index > pause[0]) return `Not checked: paused at gain ${gainAt(s, pause[0])}`;
  }
  if (halt !== null) {
    if (index > halt[0]) return `Not checked: the sweep stopped at gain ${gainAt(s, halt[0])}`;
    if (index === halt[0]) return `Not checked: ${haltText(halt[1])}`;
  }
  return "Not checked";
}

export interface GatesForm {
  damping_floor: number;
  marginal: number;
  standstill_gate: boolean;
  standstill_factor: number;
}

export function gatesBody(form: GatesForm): TuneGates {
  return {
    damping_floor: form.damping_floor,
    marginal: form.marginal,
    standstill_factor: form.standstill_gate ? form.standstill_factor : null,
  };
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
    else if (rung.waived !== null) text = `Continued anyway: ${pauseText(rung.waived.reason)}`;
    else if (verdict?.inadmissible != null) text = `Rejected: ${INADMISSIBLE_TEXT[verdict.inadmissible]}`;
    else if (verdict?.unscored != null) text = `Not scored: ${UNSCORED_TEXT[verdict.unscored]}`;
    else if (verdict?.marginal) text = "Low damping";
    else if (verdict?.score != null) text = "Accepted";
    else if (assessment !== null && !flagged && (repeatsDone >= s.repeats || pause !== null || halt !== null)) {
      text = notAssessedText(s, index, pause, halt);
    } else if (repeatsDone >= s.repeats) text = "Measured, waiting for the verdict";
    else if (repeatsDone > 0) text = `${repeatsDone} of ${s.repeats} repeats`;
    else text = "Pending";

    if (s.standstill_gate_off !== null || tune.setup.gates.standstill_factor === null)
      text = `${text} (standstill check off)`;

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
      decidingSpeed: verdict?.deciding_speed ?? null,
      gap: verdict?.gap ?? null,
      tolerance: verdict?.tolerance ?? null,
      best: assessment !== null && assessment.best === index,
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
  };
  if (isLocked(tune.state, lock)) return { ...off, restore: true };
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

export type GuidedStepId = "setup" | "velocity-p" | "velocity-i" | "position-p" | "result";

export const GUIDED_STEPS: { id: GuidedStepId; label: string }[] = [
  { id: "setup", label: "Setup" },
  { id: "velocity-p", label: "Velocity P" },
  { id: "velocity-i", label: "Velocity I corner" },
  { id: "position-p", label: "Position P" },
  { id: "result", label: "Result" },
];

const AXIS_STEP: Record<TuneAxis, GuidedStepId> = {
  VelocityP: "velocity-p",
  VelocityICornerHz: "velocity-i",
  PositionP: "position-p",
};

export const axisStepId = (axis: TuneAxis): GuidedStepId => AXIS_STEP[axis];

/** The axis a step of the guided header tunes; null for Setup and Result. */
export function axisOfStep(id: GuidedStepId): TuneAxis | null {
  return AXIS_ORDER.find((axis) => AXIS_STEP[axis] === id) ?? null;
}

/** The header step the operator is working on, from the tune's state. */
export function currentGuidedStep(tune: Tune): GuidedStepId {
  if (tune.state === "Done" || tune.state === "Abandoned") return "result";
  const last = tune.steps[tune.steps.length - 1];
  if (tune.state === "Ready") {
    const next = nextAxis(tune);
    return next === null ? "result" : AXIS_STEP[next];
  }
  return last === undefined ? "setup" : AXIS_STEP[last.axis];
}
