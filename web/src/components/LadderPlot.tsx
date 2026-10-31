import { useMemo } from "react";
import type uPlot from "uplot";
import { ladderPlotData } from "@/tune";
import { axis, PALETTE, PlotPanel, type PlotOptions } from "./PlotPanel";

const MEDIAN_COLOR = "#e5e5e5";
const NO_LINE = () => null;

/** The headline metric of every ladder run and the median per value. */
export function LadderPlot({
  entries,
  repeats,
  axisLabel,
  metricLabel,
  thresholds,
}: {
  entries: { value: number | null; metric: number | null }[];
  repeats: number;
  axisLabel: string;
  metricLabel: string;
  thresholds: { floor: number; marginal: number } | null;
}) {
  const options = useMemo<PlotOptions>(() => {
    const reps: uPlot.Series[] = Array.from({ length: repeats }, (_, r) => ({
      label: `Repeat ${r + 1}`,
      stroke: PALETTE[r % PALETTE.length],
      paths: NO_LINE,
      points: { show: true, size: 8, fill: PALETTE[r % PALETTE.length] },
    }));
    const lines: uPlot.Series[] = thresholds
      ? [
          { label: "Floor", stroke: "#a1a1a1", width: 1, dash: [4, 4], points: { show: false } },
          { label: "Marginal", stroke: "#c98500", width: 1, dash: [4, 4], points: { show: false } },
        ]
      : [];
    return {
      scales: { x: { time: false } },
      axes: [axis(axisLabel), axis(metricLabel)],
      series: [{ label: axisLabel }, ...reps, { label: "Median", stroke: MEDIAN_COLOR, width: 2, spanGaps: true, points: { show: false } }, ...lines],
    };
  }, [repeats, axisLabel, metricLabel, thresholds]);
  const data = useMemo(() => {
    const columns = ladderPlotData(entries, repeats);
    const n = columns[0].length;
    const extra = thresholds
      ? [new Array<number>(n).fill(thresholds.floor), new Array<number>(n).fill(thresholds.marginal)]
      : [];
    return [...columns, ...extra] as uPlot.AlignedData;
  }, [entries, repeats, thresholds]);
  return <PlotPanel label={`Ladder: ${metricLabel} by ${axisLabel}`} options={options} data={data} />;
}
