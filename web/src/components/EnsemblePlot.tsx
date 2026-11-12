import { useMemo, useState } from "react";
import type uPlot from "uplot";
import type { ProbePlot } from "@/api";
import { alignSeries, type Column } from "@/tune";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { axis, PlotPanel, type PlotOptions } from "./PlotPanel";
import type { OverlayRun } from "./TracePanels";

export type Signal = "velocity" | "torque" | "position_error";

const SIGNALS: { key: Signal; label: string }[] = [
  { key: "velocity", label: "Velocity" },
  { key: "torque", label: "Torque" },
  { key: "position_error", label: "Position error" },
];

const FIT_DASH = [6, 4];

const Y_LABEL: Record<Signal, string> = {
  velocity: "Speed response",
  torque: "Torque response",
  position_error: "Position error",
};

export function ensembleOptions(runs: OverlayRun<ProbePlot>[], signal: Signal): PlotOptions {
  return {
    scales: { x: { time: false } },
    axes: [axis("Time after the step (ms)"), axis(Y_LABEL[signal])],
    legend: { live: false },
    series: [
      { label: "ms" },
      ...runs.flatMap((r) => {
        const mean: uPlot.Series = { label: r.label, stroke: r.color, width: 2 };
        const fit: uPlot.Series = {
          label: `${r.label} fit`,
          stroke: r.color,
          width: 2,
          dash: FIT_DASH,
          class: "legend-hidden",
        };
        return r.value[signal].fit === null ? [mean] : [mean, fit];
      }),
    ],
  };
}

/** Ensemble-mean step responses of the checked probe runs, each with its fitted curve. */
export function EnsemblePlot({ runs }: { runs: OverlayRun<ProbePlot>[] }) {
  const [signal, setSignal] = useState<Signal>("velocity");
  const key = runs.map((r) => `${r.id}:${r.label}:${r.color}:${r.value[signal].fit === null ? 0 : 1}`).join("|");
  // biome-ignore lint/correctness/useExhaustiveDependencies: key stands in for runs, so the options rebuild only when a run's identity or fit changes.
  const options = useMemo<PlotOptions>(() => ensembleOptions(runs, signal), [key, signal]);
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
    <div className="space-y-4">
      <Tabs value={signal} onValueChange={(v) => setSignal(v as Signal)}>
        <TabsList variant="line" aria-label="Graph view">
          {SIGNALS.map((s) => (
            <TabsTrigger key={s.key} value={s.key}>
              {s.label}
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>
      <PlotPanel label={`Step-response ensembles, ${signal.replace("_", " ")}`} options={options} data={data} />
      {runs.length > missing.length && (
        <p className="text-xs text-muted-foreground">
          Solid lines are the measured average response to the probe step, dashed lines the fitted curve. foci-tune
          divides each response by the step size. Compare how far each curve overshoots and how long it takes to settle.
          The heights are not μm. The μm figures in the tables come from the tracking runs.
        </p>
      )}
      {missing.length > 0 && (
        <p className="text-xs text-muted-foreground">No fitted curve for {missing.join(", ")}: the fit failed.</p>
      )}
    </div>
  );
}
