import { ApiRequestError } from "./api";
import { failureMessage } from "./failure";
import { cornerHz, velocityIRaw } from "./units";
import type {
  Halt,
  PrinterView,
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
import { expectedProbeEdges, PID_GAIN_MAX_RAW, PROBE_MIN_EDGES, probeToggleSpacing, type ProbeEdgeInput } from "./tune";

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

export interface PrepareForm {
  /** The typed seed; null follows the stepper's live value. */
  seed: number | null;
  repeats: number;
  lower: number;
  upper: number;
  /** Divisions of an octave between neighbouring gains. */
  divisions: number;
}

export const DEFAULT_PREPARE: PrepareForm = { seed: null, repeats: 3, lower: -3, upper: 3, divisions: 3 };

/** The step sizes on offer, as divisions of an octave, from the finest to the coarsest. */
export const STEP_DIVISIONS = [6, 4, 3, 2] as const;

/** How much one step changes the gain, in percent. */
export const stepPercent = (divisions: number): number => Math.round((2 ** (1 / divisions) - 1) * 100);

export interface PrepareState {
  /** The seed the ladder starts from; null while there is neither a typed nor a live value. */
  seed: number | null;
  unit: string;
  preview: LadderPreview | null;
  /** Why the step cannot be prepared from this form, as a sentence; null when it can. */
  problem: string | null;
}

/** What the next-step form amounts to: its seed, the ladder it gives, and what stops it. */
export function prepareState(axis: TuneAxis, form: PrepareForm, control: LiveControl | null): PrepareState {
  const unit = axis === "VelocityICornerHz" ? "Hz" : "";
  const seed = form.seed ?? (control === null ? null : liveSeed(axis, control));
  const rangeOk = Number.isInteger(form.lower) && Number.isInteger(form.upper) && form.lower <= form.upper;
  const built =
    seed === null || !rangeOk
      ? null
      : previewLadder(axis, seed, control?.pwm_hz ?? 0, form.lower, form.upper, form.divisions);
  const preview = built === null || "problem" in built ? null : built;
  let problem: string | null = null;
  if (form.repeats < 1 || !Number.isInteger(form.repeats))
    problem = "Each gain must repeat at least once, in whole numbers.";
  else if (!rangeOk) problem = "The lowest and highest step must be whole numbers, lowest first.";
  else if (seed === null) problem = "There is no live value to start from. Enter a seed.";
  else if (built !== null && "problem" in built) problem = built.problem;
  else if (preview !== null && preview.gains.length === 0) {
    problem = "None of these steps gives a gain that can be written. Change the seed or the range.";
  }
  return { seed, unit, preview, problem };
}

/** The live control values of the stepper's own board and channel; null when it is not reporting. */
export function liveControlFor(printer: PrinterView, stepperName: string): LiveControl | null {
  const stepper = printer.steppers.find((s) => s.name === stepperName);
  if (stepper === undefined) return null;
  const channel = printer.boards
    .find((b) => b.serial === stepper.serial)
    ?.channels.find((c) => c.channel === stepper.channel);
  if (channel === undefined) return null;
  const { velocity_p, velocity_i, position_p, pwm_hz } = channel.control;
  return { velocity_p, velocity_i, position_p, pwm_hz };
}

export type GuidedAction =
  | "setup"
  | "prepare"
  | "adjust"
  | "start"
  | "choose"
  | "extend"
  | "decide"
  | "abandon"
  | "restore";

const OVER_REASON: Record<GuidedAction, string> = {
  abandon: "there is nothing to abandon",
  setup: "its setup can't change any more",
  restore: "there is nothing to restore",
  prepare: "nothing more can be done with it",
  adjust: "there is no step to adjust",
  start: "nothing more can be done with it",
  choose: "nothing more can be done with it",
  extend: "nothing more can be done with it",
  decide: "nothing more can be done with it",
};

/**
 * What to tell the operator when an action failed. A refusal because the tune is not as the page
 * showed it is explained from the tune as it is now; any other failure keeps the server's reason.
 */
export function failureText(action: GuidedAction, error: unknown, before: Tune | null, after: Tune | null): string {
  if (!(error instanceof ApiRequestError) || error.status !== 409 || after === null) return failureMessage(error);
  if (after.state === "Done" || after.state === "Abandoned") {
    const word = after.state === "Done" ? "finished" : "abandoned";
    return `This tune was already ${word}, so ${OVER_REASON[action]}. The page now shows its current state.`;
  }
  if (before !== null && (before.state !== after.state || before.steps.length !== after.steps.length)) {
    return "This tune changed since the page last updated, so that didn't go through. The page now shows where it is.";
  }
  return failureMessage(error);
}

/** The control values a ladder can start from. */
export interface LiveControl {
  velocity_p: number;
  velocity_i: number;
  position_p: number;
  pwm_hz: number;
}

/** The stepper's live value on an axis, in the unit the seed is entered in; null when unknown. */
export function liveSeed(axis: TuneAxis, control: LiveControl): number | null {
  switch (axis) {
    case "VelocityP":
      return control.velocity_p;
    case "PositionP":
      return control.position_p;
    case "VelocityICornerHz":
      return cornerHz(control.velocity_i, control.pwm_hz);
  }
}

export interface LadderGain {
  step: number;
  shown: number;
  raw: number;
}

export interface LadderPreview {
  /** The written value of the seed itself, whether or not the range includes it. */
  seedRaw: number;
  gains: LadderGain[];
  left: { step: number; shown: number; reason: string }[];
}

const SEED_CORNER_HZ = 50;

/**
 * The gains a ladder will try, as the service works them out: the seed times 2^(step/3), rounded
 * to the written value. Gains that cannot be written, round to zero or repeat an earlier gain are
 * left out with the reason. A seed the service would refuse is a problem in words.
 */
export function previewLadder(
  axis: TuneAxis,
  seed: number,
  pwmHz: number,
  lower: number,
  upper: number,
  divisions = 3,
): LadderPreview | { problem: string } {
  const corner = axis === "VelocityICornerHz";
  if (corner && !(pwmHz > 0)) return { problem: "The PWM frequency is unknown, so a corner can't be converted." };
  const toRaw = (shown: number) => (corner ? velocityIRaw(shown, pwmHz) : Math.round(shown));
  const writable = (shown: number, raw: number) => Number.isFinite(shown) && raw >= 0 && raw <= PID_GAIN_MAX_RAW;
  let seedRaw: number;
  if (seed === 0) {
    if (!corner) return { problem: "The seed can't be zero. Use a value above zero, or the live value." };
    seedRaw = 0;
  } else {
    seedRaw = toRaw(seed);
    if (!writable(seed, seedRaw)) return { problem: `${seed} is outside what the stepper accepts.` };
    if (seedRaw === 0)
      return { problem: "The seed is too small: it rounds to zero when written to the stepper. Use a larger value." };
  }
  const base = corner ? (seedRaw === 0 ? SEED_CORNER_HZ : (cornerHz(seedRaw, pwmHz) as number)) : seedRaw;
  const preview: LadderPreview = { seedRaw: toRaw(base), gains: [], left: [] };
  for (let step = lower; step <= upper; step += 1) {
    const shown = base * 2 ** (step / divisions);
    const raw = toRaw(shown);
    if (!writable(shown, raw)) {
      preview.left.push({ step, shown, reason: "is outside what the stepper accepts" });
    } else if (raw === 0) {
      preview.left.push({ step, shown, reason: "rounds to zero" });
    } else {
      const kept = preview.gains.find((g) => g.raw === raw);
      if (kept === undefined) preview.gains.push({ step, shown, raw });
      else preview.left.push({ step, shown, reason: `is the same gain as step ${kept.step}` });
    }
  }
  return preview;
}

/** The axis' name as the steps header shows it, without its unit. */
export const axisTitle = (axis: TuneAxis): string => GUIDED_STEPS.find((s) => s.id === AXIS_STEP[axis])?.label ?? axis;

/** What a prepared step will try, in one sentence: how many gains, their range, the start and the repeats. */
export function stepSentence(step: TuneStep): string {
  const corner = step.axis === "VelocityICornerHz";
  const gain = (shown: number, raw: number) => (corner ? `${shown.toFixed(1)} Hz` : String(raw));
  const first = step.rungs[0];
  const last = step.rungs[step.rungs.length - 1];
  const times = step.repeats === 1 ? "once" : `${step.repeats} times`;
  if (first === undefined || last === undefined) return "No gains to test.";
  if (step.rungs.length === 1) return `1 gain, ${gain(first.shown, first.raw)}, tested ${times}.`;
  const start = corner
    ? step.seed_raw === 0
      ? SEED_CORNER_HZ
      : (cornerHz(step.seed_raw, step.pwm_hz) ?? SEED_CORNER_HZ)
    : step.seed_raw;
  const from = gain(first.shown, first.raw);
  const to = gain(last.shown, last.raw);
  return `${step.rungs.length} gains from ${from} to ${to}, around the starting gain of ${corner ? `${start.toFixed(1)} Hz` : start}, each tested ${times}.`;
}

/**
 * How many runs the step makes, as the service counts them: every gain is run for each repeat as
 * a recording at each test speed, a probe and a standstill, plus one more such set when the seed's
 * own gain is in the ladder, for the drift check.
 */
export function plannedRuns(preview: LadderPreview, repeats: number, speedCount: number): number {
  const perRepeat = speedCount + 2;
  const seedInLadder = preview.gains.some((g) => g.raw === preview.seedRaw);
  return preview.gains.length * repeats * perRepeat + (seedInLadder ? perRepeat : 0);
}

/** A duration for the operator: whole minutes, or seconds for a short one, or that it is not known. */
export function durationWords(seconds: number | null): string {
  if (seconds === null) return "not known yet";
  const minutes = Math.round(seconds / 60);
  if (minutes < 1) return `${Math.round(seconds)} seconds`;
  return minutes === 1 ? "1 minute" : `${minutes} minutes`;
}

/** The next-step form a prepared step was made from, so it can be adjusted from where it was. */
export function formFromStep(step: TuneStep, live: number | null): PrepareForm {
  const corner = step.axis === "VelocityICornerHz";
  const seed = corner ? (step.seed_raw === 0 ? 0 : (cornerHz(step.seed_raw, step.pwm_hz) ?? 0)) : step.seed_raw;
  const isLive = live !== null && Math.abs(live - seed) < (corner ? 0.05 : 0.5);
  return {
    seed: isLive ? null : seed,
    repeats: step.repeats,
    lower: step.lower_k,
    upper: step.upper_k,
    divisions: step.octave_divisions,
  };
}
