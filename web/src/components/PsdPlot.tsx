import { useMemo } from "react";
import type uPlot from "uplot";
import type { PsdPlot as Psd } from "@/api";
import { alignSeries, type Column } from "@/tune";
import { axis, PlotPanel, type PlotOptions } from "./PlotPanel";
import { useStableSeries, type OverlayRun } from "./TracePanels";

/** Welch spectra of the detrended torque, log power. */
export function PsdPlot({ runs }: { runs: OverlayRun<Psd>[] }) {
  const series = useStableSeries(runs);
  const options = useMemo<PlotOptions>(
    () => ({
      scales: { x: { time: false }, y: { distr: 3 } },
      axes: [
        axis("Frequency (Hz)"),
        { ...axis("Torque power"), size: 64, values: (_u, splits) => splits.map((v) => v.toExponential(0)) },
      ],
      series: [{ label: "Hz" }, ...series],
    }),
    [series],
  );
  const data = useMemo(
    () =>
      alignSeries(
        runs.map(
          (r) => [r.value.freq_hz, r.value.power.map((p) => (p !== null && p > 0 ? p : null))] as [Column, Column],
        ),
      ) as uPlot.AlignedData,
    [runs],
  );
  return <PlotPanel label="Torque power spectrum" options={options} data={data} />;
}
