import type { Run, RunEntry } from "./api";
import {
  declaredMetrics,
  formatNumber,
  PROBE_MIN_EDGES,
  rejectionReason,
  skipReasonText,
  stateLabel,
  type GainRow,
  type RunType,
} from "./tune";
import { cornerHz } from "./units";

const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

/** Capitalises the first word of a sentence, except an identifier such as a stepper name. */
const startSentence = (text: string) => (/^\w+_\w+/.test(text) ? text : capital(text));

export function outcomeLabel(state: string): string {
  switch (state) {
    case "done":
      return "Done";
    case "not_comparable":
      return "Rejected";
    case "skipped":
      return "Did not run";
    case "failed":
      return "Failed";
    case "aborted":
      return "Cancelled";
    case "killed":
    case "kill_unconfirmed":
      return "Stopped";
    default:
      return capital(stateLabel(state));
  }
}

export function resultColumn(type: RunType): { title: string; unit: string } {
  if (type === "probe") return { title: "Damping", unit: "" };
  if (type === "tracking") return { title: "Position error", unit: "μm" };
  return { title: "Torque noise", unit: "" };
}

const GAIN_LABELS: [keyof GainRow, string][] = [
  ["velocity_p", "Velocity P"],
  ["velocity_i", "Velocity I"],
  ["position_p", "Position P"],
];

const FILTER_LABELS: [keyof GainRow, string][] = [
  ["torque_hz", "Torque"],
  ["flux_hz", "Flux"],
  ["velocity_hz", "Velocity"],
  ["position_hz", "Position"],
];

export function settingText(key: keyof GainRow, row: GainRow, pwmHz: number | null): string {
  if (key === "velocity_i") {
    return pwmHz === null ? String(row.velocity_i) : `${formatNumber(cornerHz(row.velocity_i, pwmHz), 1)} Hz`;
  }
  return key.endsWith("_hz") ? `${row[key]} Hz` : String(row[key]);
}

/**
 * The settings every run shares, as two lines for under the table title, and the keys that
 * differ between runs and so need a column. Keys in `shown` already have a column, so the
 * lines leave them out.
 */
export function settingsLines(
  rows: GainRow[],
  pwmHz: number | null,
  shown: (keyof GainRow)[] = [],
): { gains: string | null; filters: string | null; varying: (keyof GainRow)[] } {
  if (rows.length === 0) return { gains: null, filters: null, varying: [] };
  const constant = (key: keyof GainRow) => rows.every((r) => r[key] === rows[0][key]);
  const line = (title: string, items: [keyof GainRow, string][]) => {
    const parts = items
      .filter(([key]) => constant(key) && !shown.includes(key))
      .map(([key, label]) => `${label} ${settingText(key, rows[0], pwmHz)}`);
    return parts.length === 0 ? null : `${title}: ${parts.join(" · ")}`;
  };
  return {
    gains: line("Gains", GAIN_LABELS),
    filters: line("Filters", FILTER_LABELS),
    varying: [...GAIN_LABELS, ...FILTER_LABELS].map(([key]) => key).filter((key) => !constant(key)),
  };
}

/** The damping at which the bar is full. */
const FULL_BAR_DAMPING = 0.2;

/**
 * A damping value as a bar: how full, and "low" below the sweep's stop limit, "marginal"
 * below its marginal limit, otherwise "good".
 */
export function dampingBar(
  zeta: number,
  floor: number,
  marginal: number,
): { fraction: number; tone: "low" | "marginal" | "good" } {
  const fraction = Math.min(1, Math.max(0, zeta / FULL_BAR_DAMPING));
  return { fraction, tone: zeta < floor ? "low" : zeta < marginal ? "marginal" : "good" };
}

export function measuredLines(run: Run, type: RunType): string[] {
  const m = declaredMetrics(run);
  if (m === null) return ["Nothing was measured."];
  if (type === "probe") {
    const p = m.probe;
    if (p === null) return ["Nothing was measured."];
    const v = p.velocity;
    const lines =
      v.status === "fit"
        ? [`Damping: ${formatNumber(v.zeta, 3)}`, `Ringing frequency: ${formatNumber(v.damped_hz, 1)} Hz`]
        : [`Damping: could not be measured (${v.reason})`];
    lines.push(`Step responses recorded: ${p.edges} (at least ${PROBE_MIN_EDGES} are needed)`);
    if (p.torque_psd_peak_hz !== null) lines.push(`Torque noise peaks at ${formatNumber(p.torque_psd_peak_hz, 1)} Hz`);
    return lines;
  }
  if (type === "tracking") {
    if (m.tracking.length === 0) return ["Nothing was measured."];
    return [...m.tracking]
      .sort((a, b) => a.speed_mm_s - b.speed_mm_s)
      .map(
        (t) =>
          `At ${formatNumber(t.speed_mm_s, 0)} mm/s: worst 1% of position error ${formatNumber(t.p99_um, 1)} μm (typical ${formatNumber(t.sd_um, 1)} μm)`,
      );
  }
  const s = m.standstill;
  if (s === null) return ["Nothing was measured."];
  const drift = Math.max(...s.rests.map((r) => r.position_pp_um ?? Number.NaN));
  return [
    `Torque noise: ${formatNumber(s.worst_torque_sd, 1)}`,
    `Largest position drift while holding: ${formatNumber(Number.isFinite(drift) ? drift : null, 1)} μm`,
  ];
}

export function checkLines(run: Run, tested: string, others: string[]): { passed: boolean; text: string }[] {
  const moved = others.length === 1 ? others[0] : "Another motor";
  return run.verification.map((v) => {
    switch (v.check) {
      case "channel_moved":
        return {
          passed: v.passed,
          text: v.passed ? "Only the tested motor moved." : `${moved} moved while ${tested} was being tested.`,
        };
      case "baseline_contiguous":
        return { passed: v.passed, text: v.passed ? "The gains stayed as set." : "The gains changed during the run." };
      case "no_drop_gaps":
        return { passed: v.passed, text: v.passed ? "The recording is complete." : "The recording lost data." };
      case "edge_count":
        return {
          passed: v.passed,
          text: v.passed
            ? "Enough step responses were recorded."
            : "Too few step responses were recorded to measure damping.",
        };
      default:
        return { passed: v.passed, text: capital(stateLabel(v.check)) };
    }
  });
}

/** The one sentence under a run's outcome, or null for a run that needs no explanation. */
export function runOutcomeReason(entry: RunEntry, tested: string, others: string[]): string | null {
  const reason = "reason" in entry ? String(entry.reason) : null;
  if (entry.state === "done") return null;
  if (entry.state === "skipped") return reason === null ? null : skipReasonText(reason);
  if (entry.state === "not_comparable") {
    const failedChecks = (entry.run?.verification ?? []).filter((v) => !v.passed).map((v) => v.check);
    return startSentence(rejectionReason({ value: 0, state: entry.state, reason, failedChecks }, tested, others));
  }
  if (entry.state === "failed") return `${reason ?? "no reason was recorded"}.`;
  return reason === null ? null : skipReasonText(reason);
}
