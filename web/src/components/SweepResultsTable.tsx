import type { ReactNode } from "react";
import type { TuneAxis } from "@/api";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { RungView } from "@/guided";
import { cn } from "@/lib/utils";

const fmt = (n: number | null, digits = 3) => (n === null ? "-" : n.toFixed(digits));

function Explained({ tip, children }: { tip: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span tabIndex={0} className="cursor-help underline decoration-dotted">
          {children}
        </span>
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">{tip}</TooltipContent>
    </Tooltip>
  );
}

/** One row per gain of a sweep: its status, its score and the numbers the score comes from. */
export function SweepResultsTable({
  axis,
  label,
  rows,
  speeds,
}: {
  axis: TuneAxis;
  label: string;
  rows: RungView[];
  speeds: number[];
}) {
  const corner = axis === "VelocityICornerHz";
  return (
    <div className="overflow-x-auto">
      <table aria-label={`${label} results`} className="w-full text-left text-sm">
        <thead className="text-xs text-muted-foreground">
          <tr>
            <th className="pr-4 align-top font-medium">#</th>
            <th className="pr-4 align-top font-medium">Gain</th>
            <th className="pr-4 align-top font-medium">Status</th>
            <th className="pr-4 align-top font-medium">
              <Explained tip="The worst tracking error across your test speeds. Lower is better.">Score (μm)</Explained>
            </th>
            <th className="pr-4 align-top font-medium">
              <Explained tip="How much worse this gain is than the best one, with how much repeated runs of it varied.">
                Behind best
              </Explained>
            </th>
            <th className="pr-4 align-top font-medium">
              <Explained tip="How steadily the stepper settles after a sudden change. Higher is steadier; a gain below the damping stop limit is rejected.">
                Damping
              </Explained>
            </th>
            {speeds.map((s) => (
              <th key={s} className="pr-4 align-top font-medium">
                <Explained
                  tip={`99% of the time the stepper stayed within this many μm of where it should be at ${s} mm/s.`}
                >
                  {`Error at ${s} mm/s (μm)`}
                </Explained>
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="font-mono">
          {rows.map((r) => (
            <tr
              key={r.index}
              className={cn(
                r.recommended !== null && "text-emerald-300",
                r.skipped !== null && "text-muted-foreground",
              )}
            >
              <td className="pr-4 text-muted-foreground">{r.index + 1}</td>
              <td className="pr-4">{corner ? `${r.shown.toFixed(1)} Hz` : r.raw}</td>
              <td className="pr-4 font-sans">
                {r.verdict}
                {r.recommended !== null && <span className="ml-2 text-xs">({r.recommended})</span>}
              </td>
              <td className="pr-4">
                {fmt(r.score)}
                {r.decidingSpeed !== null && <span className="ml-1 text-xs"> @ {speeds[r.decidingSpeed]} mm/s</span>}
              </td>
              <td className="pr-4">
                {r.best
                  ? "best"
                  : r.gap === null
                    ? ""
                    : r.tolerance === null
                      ? `${fmt(r.gap, 1)} μm`
                      : `${fmt(r.gap, 1)} μm ± ${fmt(r.tolerance, 1)} μm`}
              </td>
              <td className="pr-4">{fmt(r.zeta)}</td>
              {speeds.map((s, i) => (
                <td key={s} className="pr-4">
                  {fmt(r.p99BySpeed[i] ?? null)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
