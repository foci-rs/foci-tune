import type { RunEntry } from "@/api";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { checkLines, measuredLines, outcomeLabel, runOutcomeReason, settingText } from "@/results";
import type { RunType } from "@/tune";
import { formatNumber } from "@/tune";
import { gainRow, pwmOf, runName, time, finishedMs } from "./ResultsTable";

export type ConfigState = { run: string; text: string } | { error: string } | null;

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-1">
      <h3 className="text-xs font-medium uppercase tracking-wider text-muted-foreground">{title}</h3>
      {children}
    </section>
  );
}

/** Everything about one run in plain words, opened from its row; a finished run can be accepted here. */
export function RunDetails({
  entry,
  type,
  ladderAxis,
  tested,
  others,
  config,
  copied,
  onAccept,
  onCopy,
  onClose,
}: {
  entry: RunEntry | null;
  type: RunType;
  ladderAxis: string | null;
  tested: string;
  others: string[];
  config: ConfigState;
  copied: boolean;
  onAccept: (run: string) => void;
  onCopy: (text: string) => void;
  onClose: () => void;
}) {
  const run = entry?.run ?? null;
  const row = run === null ? null : gainRow(run);
  const pwm = run === null ? null : pwmOf(run);
  const reason = entry === null ? null : runOutcomeReason(entry, tested, others);
  const shown = entry !== null && config !== null && "run" in config && config.run === entry.id ? config : null;
  return (
    <Sheet open={entry !== null} onOpenChange={(open) => !open && onClose()}>
      <SheetContent side="right" className="w-full overflow-y-auto p-4 sm:max-w-md">
        {entry !== null && (
          <>
            <SheetHeader className="p-0">
              <SheetTitle>Run at {runName(entry, ladderAxis)}</SheetTitle>
              <SheetDescription>
                {outcomeLabel(entry.state)}
                {reason !== null && `. ${reason}`}
              </SheetDescription>
            </SheetHeader>
            {run !== null && row !== null && (
              <div className="space-y-5 text-sm">
                <Section title="What was set">
                  <dl className="grid grid-cols-2 gap-x-4 gap-y-1">
                    {(
                      [
                        ["Velocity P", "velocity_p"],
                        ["Velocity I", "velocity_i"],
                        ["Position P", "position_p"],
                        ["Torque filter", "torque_hz"],
                        ["Flux filter", "flux_hz"],
                        ["Velocity filter", "velocity_hz"],
                        ["Position filter", "position_hz"],
                      ] as const
                    ).map(([label, key]) => (
                      <div key={key} className="contents">
                        <dt className="text-muted-foreground">{label}</dt>
                        <dd className="font-mono">
                          {settingText(key, row, pwm)}
                          {key === "velocity_i" && pwm !== null && (
                            <span className="text-muted-foreground"> (raw {row.velocity_i})</span>
                          )}
                        </dd>
                      </div>
                    ))}
                  </dl>
                </Section>
                <Section title="What was measured">
                  <ul className="space-y-0.5">
                    {measuredLines(run, type).map((line) => (
                      <li key={line}>{line}</li>
                    ))}
                  </ul>
                </Section>
                {run.verification.length > 0 && (
                  <Section title="Checks">
                    <ul className="space-y-0.5">
                      {checkLines(run, tested, others).map((c) => (
                        <li key={c.text} className={c.passed ? "text-muted-foreground" : "text-amber-400"}>
                          {c.passed ? "Passed: " : "Failed: "}
                          {c.text}
                        </li>
                      ))}
                    </ul>
                  </Section>
                )}
                <Section title="When">
                  <p>
                    Finished at {time(finishedMs(entry))}
                    {run.steps.length > 1 &&
                      `, took ${formatNumber((run.steps[run.steps.length - 1].unix_ms - run.steps[0].unix_ms) / 1000, 0)} s`}
                  </p>
                  <p className="font-mono text-xs text-muted-foreground">{entry.id}</p>
                </Section>
                {run.notes.length > 0 && (
                  <Section title="Notes">
                    <ul className="space-y-0.5 text-muted-foreground">
                      {run.notes.map((n) => (
                        <li key={n}>{n}</li>
                      ))}
                    </ul>
                  </Section>
                )}
                {(entry.state === "done" || entry.state === "not_comparable") && (
                  <div className="space-y-2">
                    {entry.state !== "done" && (
                      <p className="text-amber-400">This run was rejected; check its gains before using them.</p>
                    )}
                    <Button onClick={() => onAccept(entry.id)}>Use these settings</Button>
                    {config !== null && "error" in config && <p className="text-red-400">{config.error}</p>}
                    {shown !== null && "text" in shown && (
                      <div className="space-y-2">
                        <section aria-label="Config block">
                          <pre className="overflow-x-auto rounded-md border bg-background p-3 font-mono text-xs select-all">
                            {shown.text}
                          </pre>
                        </section>
                        <Button variant="outline" onClick={() => onCopy(shown.text)}>
                          {copied ? "Copied" : "Copy"}
                        </Button>
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}
          </>
        )}
      </SheetContent>
    </Sheet>
  );
}
