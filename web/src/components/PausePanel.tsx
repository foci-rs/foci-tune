import { useEffect, useMemo, useState } from "react";
import {
  getRun,
  getSeries,
  type FitOutcome,
  type ProbePlot,
  type RestRecord,
  type Run,
  type Series,
  type Tune,
  type TuneDecision,
} from "@/api";
import { stepSummary } from "@/guided";
import { declaredMetrics } from "@/tune";
import { EnsemblePlot } from "./EnsemblePlot";
import { PALETTE } from "./PlotPanel";
import type { OverlayRun } from "./TracePanels";
import { Button, Card } from "./ui";

const DIM_COLORS = ["#64748b", "#78716c", "#6b7280", "#71717a"];

const fixed = (n: number | null | undefined, digits = 3) => (n == null ? "-" : n.toFixed(digits));

const fitText = (f: FitOutcome) => (f.status === "fit" ? f.zeta.toFixed(3) : f.reason);

interface RepeatRef {
  rung: number;
  repeat: number;
  probeId: string;
  standstillId: string | null;
}

interface Loaded {
  runs: Record<string, Run>;
  series: Record<string, Series>;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : "request failed";
}

/** The paused rung's evidence and the operator's three decisions. */
export function PausePanel({
  tune,
  step,
  busy,
  onDecide,
}: {
  tune: Tune;
  step: number;
  busy: boolean;
  onDecide: (d: TuneDecision) => void;
}) {
  const s = tune.steps[step];
  const summary = stepSummary(tune, step);
  const pause = s.paused !== null ? ([s.paused.rung, s.paused.reason] as const) : (s.assessment?.pause ?? null);
  const pausedRung = pause === null ? null : pause[0];
  const standstillPause = pause?.[1] === "StandstillBaselineUnassessable";
  const perRepeat = tune.setup.speeds_mm_s.length + 2;

  const refs = useMemo<RepeatRef[]>(() => {
    if (pausedRung === null) return [];
    const rungs = pausedRung > 0 ? [pausedRung, pausedRung - 1] : [pausedRung];
    return rungs.flatMap((rung) => {
      const ids = s.rungs[rung]?.run_ids ?? [];
      const out: RepeatRef[] = [];
      for (let repeat = 0; repeat * perRepeat < ids.length; repeat++) {
        out.push({
          rung,
          repeat,
          probeId: ids[repeat * perRepeat],
          standstillId: ids[repeat * perRepeat + perRepeat - 1] ?? null,
        });
      }
      return out;
    });
  }, [s, pausedRung, perRepeat]);

  const idKey = refs.map((r) => `${r.probeId}/${standstillPause && r.rung === pausedRung ? r.standstillId : ""}`).join("|");
  const [loaded, setLoaded] = useState<Loaded>({ runs: {}, series: {} });
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setError(null);
    const runIds = refs.flatMap((r) =>
      standstillPause && r.rung === pausedRung && r.standstillId !== null ? [r.probeId, r.standstillId] : [r.probeId],
    );
    Promise.all([
      Promise.all(runIds.map((id) => getRun(id))),
      Promise.all(refs.map((r) => getSeries(r.probeId))),
    ]).then(
      ([runs, series]) => {
        if (!live) return;
        setLoaded({
          runs: Object.fromEntries(runIds.map((id, i) => [id, runs[i]])),
          series: Object.fromEntries(refs.map((r, i) => [r.probeId, series[i]])),
        });
      },
      (e: unknown) => live && setError(errorMessage(e)),
    );
    return () => {
      live = false;
    };
  }, [idKey]);

  if (pausedRung === null) return null;
  const medians = s.assessment?.medians;
  const speeds = tune.setup.speeds_mm_s;

  const rows = refs.map((r) => {
    const run = loaded.runs[r.probeId];
    const probe = run === undefined ? null : (declaredMetrics(run)?.probe ?? null);
    return { r, probe, loading: run === undefined };
  });

  const overlays: OverlayRun<ProbePlot>[] = refs.flatMap((r) => {
    const series = loaded.series[r.probeId];
    const run = loaded.runs[r.probeId];
    if (series === undefined) return [];
    const declared = run?.declared_channel ?? 0;
    const probe = series.channels.find((c) => c.channel === declared)?.probe ?? null;
    if (probe === null) return [];
    const current = r.rung === pausedRung;
    const label = current ? `Repeat ${r.repeat + 1}` : `Rung ${r.rung + 1}, repeat ${r.repeat + 1}`;
    const color = current ? PALETTE[r.repeat % PALETTE.length] : DIM_COLORS[r.repeat % DIM_COLORS.length];
    return [{ id: r.probeId, label, color, value: probe }];
  });

  const pausedRefs = refs.filter((r) => r.rung === pausedRung);
  const required = tune.setup.standstill.rests;
  const restsByRepeat = pausedRefs.map((r) => {
    const run = r.standstillId === null ? undefined : loaded.runs[r.standstillId];
    const rests: RestRecord[] | null = run === undefined ? null : (declaredMetrics(run)?.standstill?.rests ?? []);
    return { r, rests };
  });

  return (
    <Card label="Paused" className="border-amber-500/60">
      <p className="text-sm font-medium text-amber-300">{summary.pause}</p>
      <div className="mt-3 flex flex-wrap gap-2">
        <Button disabled={busy} onClick={() => onDecide("continue")}>
          Continue past this rung
        </Button>
        <Button disabled={busy} onClick={() => onDecide("rerun")}>
          Re-run this rung
        </Button>
        <Button disabled={busy} onClick={() => onDecide("stop")}>
          Stop the ladder
        </Button>
      </div>
      {error !== null && (
        <p role="alert" className="mt-2 text-sm text-red-400">
          Could not load the evidence: {error}
        </p>
      )}

      <div className="mt-4 overflow-x-auto">
        <table aria-label="Probe repeats" className="w-full text-left text-sm">
          <thead className="text-xs uppercase tracking-wider text-muted-foreground">
            <tr>
              <th className="pr-3">Repeat</th>
              <th className="pr-3">Rung</th>
              <th className="pr-3">Velocity fit</th>
              <th className="pr-3">Admissible starts</th>
              <th className="pr-3">Edges</th>
              <th className="pr-3">Std error at peak</th>
              <th className="pr-3">Torque zeta</th>
            </tr>
          </thead>
          <tbody className="font-mono">
            {rows.map(({ r, probe, loading }) => (
              <tr key={r.probeId} className={r.rung === pausedRung ? "" : "text-muted-foreground"}>
                <td className="pr-3">{r.repeat + 1}</td>
                <td className="pr-3">{r.rung + 1}</td>
                <td className="pr-3 font-sans">{probe === null ? (loading ? "loading" : "no probe record") : fitText(probe.velocity)}</td>
                <td className="pr-3">
                  {probe?.velocity.status === "fit" ? (probe.velocity.admissible_starts ?? "-") : "-"}
                </td>
                <td className="pr-3">{probe?.edges ?? "-"}</td>
                <td className="pr-3">{fixed(probe?.stderr_at_peak, 4)}</td>
                <td className="pr-3 font-sans">{probe === null ? "-" : fitText(probe.torque)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {medians !== undefined && (
        <div className="mt-4 overflow-x-auto">
          <table aria-label="Rung medians" className="w-full text-left text-sm">
            <thead className="text-xs uppercase tracking-wider text-muted-foreground">
              <tr>
                <th className="pr-3">Rung</th>
                {speeds.map((v) => (
                  <th key={v} className="pr-3">
                    p99 {v}
                  </th>
                ))}
                <th className="pr-3">Worst torque sd</th>
              </tr>
            </thead>
            <tbody className="font-mono">
              {(pausedRung > 0 ? [pausedRung, pausedRung - 1] : [pausedRung]).map((rung) => (
                <tr key={rung}>
                  <td className="pr-3">{rung + 1}</td>
                  {speeds.map((v, i) => (
                    <td key={v} className="pr-3">
                      {fixed(medians[rung]?.p99_by_speed[i])}
                    </td>
                  ))}
                  <td className="pr-3">{fixed(medians[rung]?.worst_torque_sd)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {standstillPause && (
        <div className="mt-4 space-y-2 text-sm">
          {restsByRepeat.map(({ r, rests }) => (
            <div key={r.probeId}>
              <p>
                Repeat {r.repeat + 1}: worst torque sd {fixed(s.rungs[pausedRung].repeat_values[r.repeat]?.worst_torque_sd)}
                {rests !== null && `, ${rests.length} of ${required} rests qualified`}
              </p>
              {rests !== null && rests.length > 0 && (
                <ul className="font-mono text-xs text-muted-foreground">
                  {rests.map((x) => (
                    <li key={x.index}>
                      rest {x.index + 1}: <span>{fixed(x.torque_sd)}</span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ))}
        </div>
      )}

      {overlays.length > 0 && (
        <div className="mt-4">
          <EnsemblePlot runs={overlays} />
        </div>
      )}
    </Card>
  );
}
