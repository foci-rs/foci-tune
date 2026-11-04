import { ApiRequestError } from "./api";

const FIELD_LABELS: Record<string, string> = {
  speeds_mm_s: "Test speeds",
  tracking_strokes: "Passes per speed",
  "probe.speed_mm_s": "Probe speed",
  "probe.high_gain": "Boost factor",
  "probe.segments_between_toggles": "Moves between gain changes",
  "probe.strokes": "Probe passes",
  "standstill.rests": "Rest periods",
  "standstill.dwell_s": "Rest length",
  "gates.damping_floor": "Damping stop limit",
  "gates.marginal": "Low damping below",
  "gates.standstill_factor": "Noise limit",
  seed: "Seed",
  pattern: "Path",
};

const REFUSALS: [RegExp, string][] = [
  [/^the printer is not fully homed$/, "The printer isn't fully homed. Home X, Y and Z, then try again."],
  [/^the printer is printing$/, "The printer is printing. Wait for the print to finish or cancel it, then try again."],
  [/^klipper is not ready$/, "Klipper isn't ready. Wait for it to start or restart it, then try again."],
  [/^a run is in flight$/, "Another run is still in progress. Wait for it to finish, then try again."],
  [
    /^the last emergency stop is unconfirmed/,
    "The last emergency stop wasn't confirmed, so motion may continue. Confirm the stop or restart Klipper, then try again.",
  ],
  [
    /^the stepper is locked until its starting gains are restored/,
    "This stepper is locked until its starting gains are restored. Restore them on the tune that locked it, then try again.",
  ],
  [
    /^the stepper has no identified electrical model$/,
    "This stepper hasn't been commissioned yet. Commission it first.",
  ],
  [
    /^the stepper's board is open but has sent no trace frame yet/,
    "The stepper's board hasn't started sending data yet. Wait a few seconds and try again.",
  ],
  [
    /^the trace interface of the stepper's board is not open$/,
    "foci-tune can't reach the stepper's board. Check its USB connection, then try again.",
  ],
  [
    /^only \d+ bytes free on the data directory$/,
    "The printer is almost out of storage. Free some space, then try again.",
  ],
];

/** What the operator reads when a request failed, in words with a next step where there is one. */
export function failureMessage(e: unknown): string {
  if (e instanceof ApiRequestError) {
    if (e.status === 0) {
      return e.message === "request timed out"
        ? "foci-tune didn't answer in time. Try again."
        : "Can't reach foci-tune. Check that the service is running, then try again.";
    }
    if (e.field !== null) return `${FIELD_LABELS[e.field] ?? e.field}: ${e.message}`;
    return REFUSALS.find(([pattern]) => pattern.test(e.message))?.[1] ?? e.message;
  }
  return e instanceof Error ? e.message : "Something went wrong. Try again.";
}
