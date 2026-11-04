import { useEffect, useMemo, useRef, useState } from "react";
import type uPlot from "uplot";
import type { RunRegime, StepperView } from "@/api";
import { useFeed } from "@/feed";
import { appendLive, liveRegime, type LiveTrace } from "@/tune";
import { SyncedPanels } from "./TracePanels";
import { PALETTE } from "./PlotPanel";

const WINDOW_S = 5;
const SERIES: uPlot.Series[] = [{ label: "Live", stroke: PALETTE[0], width: 2, spanGaps: false }];

/** The declared channel's critical samples while a run records, converted to plot units. */
export function LivePanels({ stepper, runRegime }: { stepper: StepperView; runRegime: RunRegime | null }) {
  const { listen, subscribeLive } = useFeed();
  const [trace, setTrace] = useState<LiveTrace | null>(null);
  const [error, setError] = useState<string | null>(null);
  const regimeRef = useRef(runRegime);
  regimeRef.current = runRegime;

  useEffect(() => subscribeLive(), [subscribeLive]);

  useEffect(
    () =>
      listen((message) => {
        if (message.type !== "live" || message.serial !== stepper.serial) return;
        const ch = message.channels.find((c) => c.channel === stepper.channel);
        if (ch === undefined || ch.ticks.length === 0) return;
        const regime = liveRegime(message.clock_freq, regimeRef.current, stepper);
        if (!regime.ok) {
          setError(regime.error);
          return;
        }
        setError(null);
        setTrace((t) => appendLive(t, ch, regime.regime, WINDOW_S));
      }),
    [listen, stepper],
  );

  const panels = useMemo(() => {
    const t = trace?.t_s ?? [];
    return [
      { label: "Live position error (um)", data: [t, trace?.position_error_um ?? []] as uPlot.AlignedData },
      { label: "Live velocity (mm/s)", data: [t, trace?.velocity_mm_s ?? []] as uPlot.AlignedData },
      { label: "Live torque (raw)", data: [t, trace?.torque ?? []] as uPlot.AlignedData },
    ];
  }, [trace]);

  return (
    <div className="space-y-2">
      {error !== null && <p className="text-sm text-amber-400">Live samples cannot be shown: {error}</p>}
      {trace === null && error === null && <p className="text-sm text-muted-foreground">Waiting for live samples</p>}
      <SyncedPanels syncKey="live" panels={panels} series={SERIES} xLabel="Time (s)" />
    </div>
  );
}
