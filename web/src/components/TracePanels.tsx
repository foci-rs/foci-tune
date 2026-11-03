import { useCallback, useMemo, useRef } from "react";
import type uPlot from "uplot";
import type { ChannelPlot } from "@/api";
import { alignSeries, type Column } from "@/tune";
import { axis, PlotPanel, type PlotOptions } from "./PlotPanel";

export interface OverlayRun<T> {
  id: string;
  label: string;
  color: string;
  value: T;
}

type Signal = "position_error_um" | "velocity_mm_s" | "torque";

export const TRACE_SIGNALS: { key: Signal; label: string }[] = [
  { key: "position_error_um", label: "Position error (μm)" },
  { key: "velocity_mm_s", label: "Velocity (mm/s)" },
  { key: "torque", label: "Torque (raw)" },
];

/** Stacked panels sharing one cursor and one x zoom. */
export function SyncedPanels({
  syncKey,
  panels,
  series,
  xLabel,
}: {
  syncKey: string;
  panels: { label: string; data: uPlot.AlignedData }[];
  series: uPlot.Series[];
  xLabel: string;
}) {
  const plots = useRef(new Set<uPlot>());
  const syncing = useRef(false);
  const panelKey = panels.map((p) => p.label).join("|");

  const options = useMemo<PlotOptions[]>(
    () =>
      panels.map((p) => ({
        cursor: { sync: { key: syncKey } },
        scales: { x: { time: false } },
        axes: [axis(xLabel), axis(p.label)],
        series: [{ label: xLabel }, ...series],
        hooks: {
          setScale: [
            (u: uPlot, key: string) => {
              if (key !== "x" || syncing.current) return;
              const { min, max } = u.scales.x;
              if (min === undefined || max === undefined) return;
              syncing.current = true;
              plots.current.forEach((other) => {
                if (other !== u) other.setScale("x", { min, max });
              });
              syncing.current = false;
            },
          ],
        },
      })),
    [syncKey, xLabel, series, panelKey],
  );

  const track = useRef(new Map<number, uPlot>());
  const onCreate = useCallback(
    (k: number) => (plot: uPlot | null) => {
      const old = track.current.get(k);
      if (old !== undefined) plots.current.delete(old);
      if (plot === null) track.current.delete(k);
      else {
        track.current.set(k, plot);
        plots.current.add(plot);
      }
    },
    [],
  );
  const callbacks = useMemo(() => panels.map((_, k) => onCreate(k)), [panels.length, onCreate]);

  return (
    <div className="space-y-2">
      {panels.map((p, k) => (
        <PlotPanel
          key={p.label}
          label={p.label}
          options={options[k]}
          data={p.data}
          height={180}
          onCreate={callbacks[k]}
        />
      ))}
    </div>
  );
}

export function runSeries(runs: { label: string; color: string }[], dash?: number[]): uPlot.Series[] {
  return runs.map((r) => ({ label: r.label, stroke: r.color, width: 2, dash, spanGaps: false }));
}

export function useStableSeries(runs: { id: string; label: string; color: string }[], dash?: number[]) {
  const key = runs.map((r) => `${r.id}:${r.label}:${r.color}`).join("|");
  const latest = useRef(runs);
  latest.current = runs;
  return useMemo(() => runSeries(latest.current, dash), [key, dash]);
}

/** Position error, velocity and torque over time for the checked runs. */
export function TracePanels({ runs }: { runs: OverlayRun<ChannelPlot>[] }) {
  const series = useStableSeries(runs);
  const panels = useMemo(
    () =>
      TRACE_SIGNALS.map((s) => ({
        label: s.label,
        data: alignSeries(runs.map((r) => [r.value.t_s, r.value[s.key]] as [Column, Column])) as uPlot.AlignedData,
      })),
    [runs],
  );
  return <SyncedPanels syncKey="trace" panels={panels} series={series} xLabel="Time (s)" />;
}
