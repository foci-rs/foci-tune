import { useEffect, useRef } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";

export type PlotOptions = Omit<uPlot.Options, "width" | "height">;

export const PALETTE = [
  "#3987e5",
  "#d95926",
  "#199e70",
  "#c98500",
  "#d55181",
  "#008300",
  "#9085e9",
  "#e66767",
] as const;

export const MAX_OVERLAY = PALETTE.length;

const AXIS_INK = "#a1a1a1";
const GRID = "rgba(255,255,255,0.08)";

export function axis(label: string): uPlot.Axis {
  return {
    label,
    stroke: AXIS_INK,
    grid: { stroke: GRID, width: 1 },
    ticks: { stroke: GRID, width: 1 },
    labelSize: 18,
    size: 56,
  };
}

/**
 * One uPlot chart. `options` must be memoised by the caller: a new object rebuilds the chart,
 * new `data` only redraws it.
 */
export function PlotPanel({
  label,
  options,
  data,
  height = 200,
  onCreate,
}: {
  label: string;
  options: PlotOptions;
  data: uPlot.AlignedData;
  height?: number;
  onCreate?: (plot: uPlot | null) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const latest = useRef(data);
  latest.current = data;

  useEffect(() => {
    const el = container.current;
    if (el === null) return;
    const width = Math.max(el.clientWidth, 200);
    const instance = new uPlot({ ...options, width, height }, latest.current, el);
    plot.current = instance;
    onCreate?.(instance);
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => {
            instance.setSize({ width: Math.max(el.clientWidth, 200), height });
          });
    observer?.observe(el);
    return () => {
      observer?.disconnect();
      onCreate?.(null);
      instance.destroy();
      plot.current = null;
    };
  }, [options, height, onCreate]);

  useEffect(() => {
    plot.current?.setData(data);
  }, [data]);

  return <figure aria-label={label} ref={container} className="w-full" />;
}
