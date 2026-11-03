import type { Run, RunEntry } from "@/api";
import { cn } from "@/lib/utils";
import {
  declaredMetrics,
  formatNumber,
  headlineMetric,
  highlightChanges,
  stateLabel,
  type GainRow,
  type RunType,
} from "@/tune";
import { cornerHz } from "@/units";
import { ladderAxisLabel } from "./GainsBlock";
import { PALETTE } from "./PlotPanel";

const GAIN_COLUMNS: { key: keyof GainRow; label: string }[] = [
  { key: "velocity_p", label: "Vel P" },
  { key: "velocity_i", label: "Vel I" },
  { key: "position_p", label: "Pos P" },
  { key: "velocity_hz", label: "Vel filt" },
  { key: "torque_hz", label: "Torq filt" },
  { key: "position_hz", label: "Pos filt" },
  { key: "flux_hz", label: "Flux filt" },
];

function gainRow(run: Run): GainRow {
  const { gains, filters } = run.request;
  return {
    velocity_p: gains.velocity_p,
    velocity_i: gains.velocity_i,
    position_p: gains.position_p,
    ...filters,
  };
}

export function finishedMs(entry: RunEntry): number {
  const steps = entry.run?.steps ?? [];
  return steps.length > 0 ? steps[steps.length - 1].unix_ms : (entry.run?.created_unix_s ?? 0) * 1000;
}

function pwmOf(run: Run): number | null {
  const pwm = run.regime?.pwm_hz ?? run.baselines.find((b) => b.channel === run.declared_channel)?.control.pwm_hz;
  return pwm !== undefined && pwm > 0 ? pwm : null;
}

function time(unixMs: number): string {
  return new Date(unixMs).toLocaleTimeString([], { hour12: false });
}

function Details({ run, type }: { run: Run; type: RunType }) {
  const m = declaredMetrics(run);
  if (m === null) return <span className="text-muted-foreground">no metrics</span>;
  if (type === "tracking") {
    return (
      <span>
        {[...m.tracking]
          .sort((a, b) => a.speed_mm_s - b.speed_mm_s)
          .map(
            (t) =>
              `${formatNumber(t.speed_mm_s, 0)}: p99 ${formatNumber(t.p99_um, 1)}, rev ${formatNumber(t.reversal_peak_um, 1)}`,
          )
          .join("; ")}
      </span>
    );
  }
  if (type === "probe") {
    const p = m.probe;
    if (p === null) return <span className="text-muted-foreground">no probe metrics</span>;
    const v = p.velocity;
    return (
      <span>
        {v.status === "fit" ? `${formatNumber(v.damped_hz, 1)} Hz` : `fit failed: ${v.reason}`}, {p.edges} edges, PSD
        peak {formatNumber(p.torque_psd_peak_hz, 1)} Hz
      </span>
    );
  }
  const worstPp = Math.max(...(m.standstill?.rests ?? []).map((r) => r.position_pp_um ?? Number.NaN));
  return <span>max pos pp {formatNumber(Number.isFinite(worstPp) ? worstPp : null, 1)} μm</span>;
}

function StateCell({ entry }: { entry: RunEntry }) {
  const tone =
    entry.state === "done"
      ? "text-emerald-400"
      : entry.state === "failed" || entry.state === "kill_unconfirmed" || entry.state === "killed"
        ? "text-red-400"
        : "text-amber-400";
  const failedChecks = entry.run?.verification.filter((v) => !v.passed) ?? [];
  return (
    <div>
      <span className={tone}>{stateLabel(entry.state)}</span>
      {"reason" in entry && <p className="text-xs text-muted-foreground">{entry.reason}</p>}
      {failedChecks.map((v) => (
        <p key={v.check} className="text-xs text-muted-foreground">
          {stateLabel(v.check)}: {v.detail}
        </p>
      ))}
      {entry.run?.notes.map((n) => (
        <p key={n} className="text-xs text-muted-foreground">
          {n}
        </p>
      ))}
    </div>
  );
}

/** One row per finished run in the order they finished; gains that differ from the previous row are highlighted. */
export function ResultsTable({
  entries: unsorted,
  type,
  ladderAxis,
  checked,
  colors,
  onToggle,
}: {
  entries: RunEntry[];
  type: RunType;
  ladderAxis: string | null;
  checked: ReadonlySet<string>;
  colors: ReadonlyMap<string, number>;
  onToggle: (id: string, on: boolean) => void;
}) {
  if (unsorted.length === 0) {
    return <p className="text-sm text-muted-foreground">No finished runs in this session yet.</p>;
  }
  const entries = [...unsorted].sort((a, b) => finishedMs(a) - finishedMs(b));
  const sessionPwm = entries.map((e) => (e.run === null ? null : pwmOf(e.run))).find((p) => p !== null) ?? null;
  const marks = highlightChanges(entries.flatMap((e) => (e.run === null ? [] : [gainRow(e.run)])));
  let markIndex = 0;
  const full = checked.size >= PALETTE.length;
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="text-xs uppercase tracking-wider text-muted-foreground">
          <tr>
            <th className="py-1 pr-2 font-medium">
              <span className="sr-only">Overlay</span>
            </th>
            <th className="py-1 pr-3 font-medium">Time</th>
            {GAIN_COLUMNS.map((c) => (
              <th key={c.key} className="py-1 pr-3 text-right font-medium">
                {c.label}
              </th>
            ))}
            {ladderAxis !== null && <th className="py-1 pr-3 text-right font-medium">{ladderAxisLabel(ladderAxis)}</th>}
            <th className="py-1 pr-3 text-right font-medium">Headline</th>
            <th className="py-1 pr-3 font-medium">Metrics</th>
            <th className="py-1 pr-3 font-medium">State</th>
          </tr>
        </thead>
        <tbody className="font-mono">
          {entries.map((entry) => {
            const run = entry.run;
            if (run === null) {
              return (
                <tr key={entry.id} className="border-t">
                  <td />
                  <td className="py-1 pr-3" colSpan={GAIN_COLUMNS.length + (ladderAxis !== null ? 4 : 3)}>
                    {entry.id}: incomplete run directory
                  </td>
                  <td className="py-1 pr-3">
                    <StateCell entry={entry} />
                  </td>
                </tr>
              );
            }
            const changed = marks[markIndex++];
            const row = gainRow(run);
            const head = headlineMetric(run, type);
            const slot = colors.get(entry.id);
            const isChecked = checked.has(entry.id);
            const pwm = pwmOf(run) ?? sessionPwm;
            return (
              <tr key={entry.id} className="border-t align-top">
                <td className="py-1 pr-2">
                  <input
                    type="checkbox"
                    aria-label={`Overlay run ${time(finishedMs(entry))}`}
                    checked={isChecked}
                    disabled={!isChecked && full}
                    onChange={(e) => onToggle(entry.id, e.target.checked)}
                  />
                  {slot !== undefined && (
                    <span
                      aria-hidden="true"
                      className="ml-1 inline-block size-2 rounded-sm"
                      style={{ background: PALETTE[slot] }}
                    />
                  )}
                </td>
                <td className="py-1 pr-3">{time(finishedMs(entry))}</td>
                {GAIN_COLUMNS.map((c) => (
                  <td
                    key={c.key}
                    className={cn(
                      "py-1 pr-3 text-right",
                      changed.has(c.key) && "rounded bg-emerald-900/50 text-emerald-200",
                    )}
                  >
                    {c.key === "velocity_i" && pwm !== null
                      ? `${formatNumber(cornerHz(row.velocity_i, pwm), 1)} Hz (${row.velocity_i})`
                      : row[c.key]}
                    {changed.has(c.key) && <span className="sr-only"> changed</span>}
                  </td>
                ))}
                {ladderAxis !== null && (
                  <td className="py-1 pr-3 text-right">{formatNumber(run.request.ladder_value, 2)}</td>
                )}
                <td className="py-1 pr-3 text-right" title={head.label}>
                  {formatNumber(head.value, type === "probe" ? 3 : 1)}
                  {head.value !== null && head.unit !== "" && (
                    <span className="text-muted-foreground"> {head.unit}</span>
                  )}
                </td>
                <td className="py-1 pr-3 font-sans text-xs">
                  <Details run={run} type={type} />
                  {run.clamped_speeds.length > 0 && (
                    <p className="text-amber-400">
                      clamped:{" "}
                      {run.clamped_speeds
                        .map(([req, app]) => `${formatNumber(req, 0)} to ${formatNumber(app, 0)} mm/s`)
                        .join(", ")}
                    </p>
                  )}
                </td>
                <td className="py-1 pr-3 font-sans">
                  <StateCell entry={entry} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
