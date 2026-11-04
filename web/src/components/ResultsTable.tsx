import { ChevronDownIcon } from "lucide-react";
import { Checkbox } from "@/components/ui/checkbox";
import type { Run, RunEntry } from "@/api";
import { Button } from "@/components/ui/button";
import { dampingBar, outcomeLabel, resultColumn, runOutcomeReason, settingText, settingsLines } from "@/results";
import { formatNumber, headlineMetric, type GainRow, type RunType } from "@/tune";
import { ladderAxisLabel } from "./GainsBlock";
import { PALETTE } from "./PlotPanel";

const COLUMN_LABEL: Record<keyof GainRow, string> = {
  velocity_p: "Velocity P",
  velocity_i: "Velocity I",
  position_p: "Position P",
  torque_hz: "Torque filter",
  flux_hz: "Flux filter",
  velocity_hz: "Velocity filter",
  position_hz: "Position filter",
};

const ALL_KEYS: (keyof GainRow)[] = [
  "velocity_p",
  "velocity_i",
  "position_p",
  "torque_hz",
  "flux_hz",
  "velocity_hz",
  "position_hz",
];

/** Always a column, so the table is never bare: the two gains a user tunes by hand. */
const ALWAYS_SHOWN: (keyof GainRow)[] = ["velocity_p", "position_p"];

/** The axis a sweep varies, as the gain it sets. */
const SWEPT_KEY: Record<string, keyof GainRow> = {
  velocity_p: "velocity_p",
  velocity_i_corner_hz: "velocity_i",
  position_p: "position_p",
};

export function gainRow(run: Run): GainRow {
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

export function pwmOf(run: Run): number | null {
  const pwm = run.regime?.pwm_hz ?? run.baselines.find((b) => b.channel === run.declared_channel)?.control.pwm_hz;
  return pwm !== undefined && pwm > 0 ? pwm : null;
}

export function time(unixMs: number): string {
  return new Date(unixMs).toLocaleTimeString([], { hour12: false });
}

/** What names a run in the table and its panel: its sweep value, or when it finished. */
export function runName(entry: RunEntry, ladderAxis: string | null): string {
  const value = entry.run?.request.ladder_value ?? null;
  if (ladderAxis !== null && value !== null) return `${value}${ladderAxis === "velocity_i_corner_hz" ? " Hz" : ""}`;
  return time(finishedMs(entry));
}

const BAR_TONE = { low: "bg-red-400", marginal: "bg-amber-400", good: "bg-emerald-500" } as const;

function DampingBar({ zeta, limits }: { zeta: number; limits: { floor: number; marginal: number } }) {
  const { fraction, tone } = dampingBar(zeta, limits.floor, limits.marginal);
  return (
    <div
      role="meter"
      aria-label={`Damping ${formatNumber(zeta, 3)}`}
      aria-valuemin={0}
      aria-valuemax={1}
      aria-valuenow={zeta}
      data-tone={tone}
      title={
        tone === "low"
          ? `Below the stop limit of ${limits.floor}`
          : tone === "marginal"
            ? `Below ${limits.marginal}, which counts as marginal`
            : "Well damped"
      }
      className="h-2 w-16 overflow-hidden rounded-full bg-muted"
    >
      <div className={`h-full ${BAR_TONE[tone]}`} style={{ width: `${fraction * 100}%` }} />
    </div>
  );
}

function OutcomeCell({ entry, reason }: { entry: RunEntry; reason: string | null }) {
  const tone =
    entry.state === "done"
      ? "text-emerald-400"
      : entry.state === "failed" || entry.state === "kill_unconfirmed" || entry.state === "killed"
        ? "text-red-400"
        : "text-amber-400";
  return (
    <div>
      <span className={tone}>{outcomeLabel(entry.state)}</span>
      {reason !== null && <p className="max-w-xs text-xs text-muted-foreground">{reason}</p>}
    </div>
  );
}

/**
 * One short row per finished run, in the order it is given. What never changes between runs
 * sits in two lines above the table; the full detail of a run opens from its Details button.
 */
export function ResultsTable({
  entries,
  type,
  ladderAxis,
  checked,
  colors,
  tested,
  others,
  limits,
  onToggle,
  onOpen,
}: {
  entries: RunEntry[];
  type: RunType;
  ladderAxis: string | null;
  checked: ReadonlySet<string>;
  colors: ReadonlyMap<string, number>;
  tested: string;
  others: string[];
  /** The damping below which a probe is marginal, and the stop limit, for the bar. */
  limits: { floor: number; marginal: number };
  onToggle: (id: string, on: boolean) => void;
  onOpen: (id: string) => void;
}) {
  if (entries.length === 0) {
    return <p className="text-sm text-muted-foreground">No finished runs in this session yet.</p>;
  }
  const sessionPwm = entries.map((e) => (e.run === null ? null : pwmOf(e.run))).find((p) => p !== null) ?? null;
  const ran = entries.flatMap((e) => (e.run === null || e.state === "skipped" ? [] : [e.run]));
  const { gains, filters, varying } = settingsLines(ran.map(gainRow), sessionPwm, ALWAYS_SHOWN);
  const swept = ladderAxis === null ? null : SWEPT_KEY[ladderAxis];
  const columns = ALL_KEYS.filter((key) => key === swept || ALWAYS_SHOWN.includes(key) || varying.includes(key));
  const result = resultColumn(type);
  const full = checked.size >= PALETTE.length;
  return (
    <div className="space-y-3">
      <div className="space-y-0.5 text-sm text-muted-foreground">
        {gains !== null && <p>{gains}</p>}
        {filters !== null && <p>{filters}</p>}
      </div>
      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="text-xs uppercase tracking-wider text-muted-foreground">
            <tr>
              <th className="py-1 pr-2 font-medium">
                <span className="sr-only">Overlay</span>
              </th>
              <th className="py-1 pr-4 font-medium">Time</th>
              {columns.map((key) => (
                <th key={key} className="py-1 pr-4 text-right font-medium">
                  {key === swept && ladderAxis !== null ? ladderAxisLabel(ladderAxis) : COLUMN_LABEL[key]}
                </th>
              ))}
              <th className="py-1 pr-4 text-right font-medium">{result.title}</th>
              <th className="py-1 pr-4 font-medium">Outcome</th>
              <th className="py-1 font-medium">
                <span className="sr-only">Details</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {entries.map((entry) => {
              const run = entry.run;
              if (run === null) {
                return (
                  <tr key={entry.id} className="border-t">
                    <td />
                    <td className="py-2 pr-4" colSpan={columns.length + 2}>
                      {entry.id}: incomplete run directory
                    </td>
                    <td className="py-2 pr-4">
                      <OutcomeCell entry={entry} reason={runOutcomeReason(entry, tested, others)} />
                    </td>
                    <td />
                  </tr>
                );
              }
              const row = gainRow(run);
              const head = headlineMetric(run, type);
              const slot = colors.get(entry.id);
              const isChecked = checked.has(entry.id);
              const pwm = pwmOf(run) ?? sessionPwm;
              const name = runName(entry, ladderAxis);
              return (
                <tr key={entry.id} className="border-t align-top">
                  <td className="py-2 pr-2">
                    <Checkbox
                      aria-label={`Overlay run ${time(finishedMs(entry))}`}
                      checked={isChecked}
                      disabled={entry.state === "skipped" || (!isChecked && full)}
                      onCheckedChange={(on) => onToggle(entry.id, on === true)}
                      className="align-middle"
                    />
                    {slot !== undefined && (
                      <span
                        aria-hidden="true"
                        className="ml-1 inline-block size-2 rounded-sm"
                        style={{ background: PALETTE[slot] }}
                      />
                    )}
                  </td>
                  <td className="py-2 pr-4 text-muted-foreground">{time(finishedMs(entry))}</td>
                  {columns.map((key) => (
                    <td key={key} className="py-2 pr-4 text-right font-mono">
                      {key === swept ? (run.request.ladder_value === null ? "–" : name) : settingText(key, row, pwm)}
                    </td>
                  ))}
                  <td className="py-2 pr-4 text-right font-mono" title={head.label}>
                    <div className="flex items-center justify-end gap-2">
                      {head.value === null ? "–" : formatNumber(head.value, type === "probe" ? 3 : 1)}
                      {head.value !== null && result.unit !== "" && (
                        <span className="text-muted-foreground">{result.unit}</span>
                      )}
                      {type === "probe" && head.value !== null && <DampingBar zeta={head.value} limits={limits} />}
                    </div>
                  </td>
                  <td className="py-2 pr-4">
                    <OutcomeCell entry={entry} reason={runOutcomeReason(entry, tested, others)} />
                  </td>
                  <td className="py-1">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Details for ${name}`}
                      onClick={() => onOpen(entry.id)}
                    >
                      <ChevronDownIcon />
                    </Button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
