import { useMemo } from "react";
import type uPlot from "uplot";
import { curveData, type RecommendationKind, type RungView } from "@/guided";
import { axis, PALETTE, PlotPanel, type PlotOptions } from "./PlotPanel";

const MARK = "#34d399";

/** Tracking p99 per speed against the rung gain, with the recommendation marked on it. */
export function GuidedCurve({
  rows,
  speeds,
  axisLabel,
  recommended,
}: {
  rows: RungView[];
  speeds: number[];
  axisLabel: string;
  recommended: { gain: number; kind: RecommendationKind } | null;
}) {
  const options = useMemo<PlotOptions>(
    () => ({
      scales: { x: { time: false } },
      axes: [axis(axisLabel), axis("p99 position error")],
      series: [
        { label: axisLabel },
        ...speeds.map((s, i) => ({
          label: `${s} mm/s`,
          stroke: PALETTE[i % PALETTE.length],
          width: 2,
          spanGaps: true,
          points: { show: true, size: 6, fill: PALETTE[i % PALETTE.length] },
        })),
      ],
      hooks: {
        draw: [
          (u: uPlot) => {
            if (recommended === null) return;
            const x = u.valToPos(recommended.gain, "x", true);
            const { top, height } = u.bbox;
            const ctx = u.ctx;
            ctx.save();
            ctx.strokeStyle = MARK;
            ctx.lineWidth = 2;
            ctx.setLineDash(recommended.kind === "interpolated" ? [6, 4] : []);
            ctx.beginPath();
            ctx.moveTo(x, top);
            ctx.lineTo(x, top + height);
            ctx.stroke();
            ctx.restore();
          },
        ],
      },
    }),
    [speeds, axisLabel, recommended],
  );
  const data = useMemo(() => curveData(rows, speeds.length) as uPlot.AlignedData, [rows, speeds.length]);
  return <PlotPanel label={`p99 by ${axisLabel}`} options={options} data={data} height={220} />;
}
