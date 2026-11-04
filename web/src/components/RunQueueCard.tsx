import type { CurrentRunView, StepTime } from "@/api";
import { RUN_STEPS, runProgress, runStatusLabel, stateLabel, type RunType } from "@/tune";
import { Card } from "./ui";

const capital = (text: string) => text.charAt(0).toUpperCase() + text.slice(1);

export interface QueuedRun {
  id: string;
  state: string;
  value: number | null;
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline gap-3">
      <dt className="w-16 shrink-0 text-[0.65rem] uppercase tracking-wider text-muted-foreground">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </div>
  );
}

/** The run in flight, how far along it is, and the runs waiting behind it. */
export function RunQueueCard({
  current,
  kind,
  otherSession,
  position,
  steps,
  predictedS,
  nowMs,
  pending,
  queued,
  sweep,
  showQueue = true,
}: {
  current: CurrentRunView | null;
  /** What kind of run it is, which decides what its motion stage is called. */
  kind?: RunType;
  otherSession: boolean;
  /** Which run of a multi-run session this is. */
  position: { run: number; of: number } | null;
  /** When each stage of the run started, and the motion's predicted length. */
  steps: StepTime[];
  predictedS: number | null;
  nowMs: number;
  pending: QueuedRun[];
  queued: number;
  /** The runs of the whole sweep: how many are finished and how many there will be. */
  sweep?: { done: number; total: number };
  showQueue?: boolean;
}) {
  const progress = current === null ? null : runProgress({ state: current.state, steps, predictedS, nowMs });
  return (
    <Card label="Run queue">
      <dl className="space-y-2 text-sm">
        {current !== null && (
          <>
            <Row label="Run">
              <span className="font-mono">{current.run}</span>
              {position !== null && (
                <span className="ml-2 text-muted-foreground">
                  Run {position.run} of {position.of}
                </span>
              )}
              {otherSession && <span className="ml-2 text-muted-foreground">in another session</span>}
            </Row>
            <Row label="Status">
              <span className="text-emerald-300">{capital(runStatusLabel(current.state, kind))}</span>
              {"reason" in current && <span className="ml-2 text-muted-foreground">{current.reason}</span>}
            </Row>
            {progress !== null && (
              <Row label="Progress">
                <div className="flex items-center gap-3">
                  <div
                    role="progressbar"
                    aria-label="Run progress"
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.round(progress.fraction * 100)}
                    className="h-2 w-48 overflow-hidden rounded-full bg-muted"
                  >
                    <div className="h-full bg-emerald-500" style={{ width: `${progress.fraction * 100}%` }} />
                  </div>
                  <span className="text-muted-foreground">
                    Step {progress.step} of {RUN_STEPS.length}
                    {progress.secondsLeft === null
                      ? ""
                      : progress.secondsLeft === 0
                        ? " · finishing"
                        : ` · about ${progress.secondsLeft} s left`}
                  </span>
                </div>
              </Row>
            )}
          </>
        )}
        {sweep !== undefined && sweep.total > 0 && (
          <Row label="Sweep">
            <div className="flex items-center gap-3">
              <div
                role="progressbar"
                aria-label="Sweep progress"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.round(Math.min(1, (sweep.done + (progress?.fraction ?? 0)) / sweep.total) * 100)}
                className="h-2 w-48 overflow-hidden rounded-full bg-muted"
              >
                <div
                  className="h-full bg-emerald-500"
                  style={{ width: `${Math.min(1, (sweep.done + (progress?.fraction ?? 0)) / sweep.total) * 100}%` }}
                />
              </div>
              <span className="text-muted-foreground">
                {sweep.done} of {sweep.total} runs
              </span>
            </div>
          </Row>
        )}
        {showQueue && (
          <Row label="Queue">
            <span className="text-muted-foreground">{queued} queued</span>
            {pending.length > 0 && (
              <ol aria-label="Queue" className="mt-1 flex flex-wrap gap-2 text-xs">
                {pending.map((r) => (
                  <li key={r.id} className="rounded border px-2 py-0.5 font-mono">
                    {r.value !== null ? `${r.value} · ` : ""}
                    {stateLabel(r.state)}
                  </li>
                ))}
              </ol>
            )}
          </Row>
        )}
      </dl>
    </Card>
  );
}
