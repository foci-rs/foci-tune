import { useMemo, useState } from "react";
import type uPlot from "uplot";
import type { ProbePlot } from "@/api";
import { alignSeries, type Column } from "@/tune";
import { axis, PlotPanel, type PlotOptions } from "./PlotPanel";
import type { OverlayRun } from "./TracePanels";

type Signal = "velocity" | "torque" | "position_error";

const SIGNALS: { key: Signal; label: string }[] = [
  { key: "velocity", label: "Velocity" },
  { key: "torque", label: "Torque" },
  { key: "position_error", label: "Position error" },
];

const FIT_DASH = [6, 4];

/** Ensemble-mean step responses of the checked probe runs, each with its fitted curve. */
export function EnsemblePlot({ runs }: { runs: OverlayRun<ProbePlot>[] }) {
  const [signal, setSignal] = useState<Signal>("velocity");
  const key = runs.map((r) => `${r.id}:${r.label}:${r.color}:${r.value[signal].fit === null ? 0 : 1}`).join("|");
  const options = useMemo<PlotOptions>(
    () => ({
      scales: { x: { time: false } },
      axes: [axis("Time after edge (ms)"), axis("Per offset step")],
      series: [
        { label: "ms" },
        ...runs.flatMap((r) => {
          const mean: uPlot.Series = { label: `${r.label} mean`, stroke: r.color, width: 2 };
          const fit: uPlot.Series = { label: `${r.label} fit`, stroke: r.color, width: 2, dash: FIT_DASH };
          return r.value[signal].fit === null ? [mean] : [mean, fit];
        }),
      ],
    }),
    [key, signal],
  );
  const data = useMemo(
    () =>
      alignSeries(
        runs.map((r) => {
          const e = r.value[signal];
          const t = r.value.t_s.map((s) => s * 1000);
          return (e.fit === null ? [t, e.mean] : [t, e.mean, e.fit]) as [Column, ...Column[]];
        }),
      ) as uPlot.AlignedData,
    [runs, signal],
  );
  const missing = runs.filter((r) => r.value[signal].fit === null).map((r) => r.label);
  return (
    <div className="space-y-2">
      <div role="radiogroup" aria-label="Ensemble signal" className="flex gap-1">
        {SIGNALS.map((s) => (
          <button
            key={s.key}
            type="button"
            role="radio"
            aria-checked={signal === s.key}
            onClick={() => setSignal(s.key)}
            className={
              signal === s.key
                ? "rounded-full border border-emerald-500 px-3 py-0.5 text-xs text-emerald-300"
                : "rounded-full border px-3 py-0.5 text-xs text-muted-foreground"
            }
          >
            {s.label}
          </button>
        ))}
      </div>
      <PlotPanel label={`Step-response ensembles, ${signal.replace("_", " ")}`} options={options} data={data} />
      {missing.length > 0 && (
        <p className="text-xs text-muted-foreground">No fitted curve for {missing.join(", ")}: the fit failed.</p>
      )}
    </div>
  );
}
