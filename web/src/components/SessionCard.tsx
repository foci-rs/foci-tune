import { RotateCcwIcon } from "lucide-react";
import type { SessionView } from "@/api";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { runTypeOfKind } from "@/tune";
import { Card } from "./ui";

const TYPE_LABEL = { tracking: "Tracking", probe: "Probe", standstill: "Standstill" } as const;

/**
 * The session a run belongs to: its setup, the gains it started with, and the way back to them.
 * Shown full width above that session's results.
 */
export function SessionCard({
  view,
  canRestore,
  onRestore,
  newSessionNext,
  notes,
}: {
  view: SessionView;
  canRestore: boolean;
  onRestore: () => void;
  newSessionNext: boolean;
  /** What happened to the session's runs, such as a rejected run or a sweep that stopped. */
  notes: string[];
}) {
  const session = view.session;
  if (session === null) return null;
  const { start, end } = session.pattern;
  const { gains } = session;
  return (
    <Card
      label={`Session ${session.name}`}
      action={
        <div className="flex items-center gap-2">
          {newSessionNext && (
            <Tooltip>
              <TooltipTrigger asChild>
                <span tabIndex={0} className="rounded-full border px-2 py-0.5 text-xs text-muted-foreground">
                  New session on next run
                </span>
              </TooltipTrigger>
              <TooltipContent side="bottom" align="end" className="max-w-xs">
                Changing the setup starts a new session, so the next run's results do not join this one.
              </TooltipContent>
            </Tooltip>
          )}
          <Button variant="outline" size="sm" disabled={!canRestore} onClick={onRestore}>
            <RotateCcwIcon />
            Restore starting values
          </Button>
        </div>
      }
    >
      <div className="space-y-1 text-sm text-muted-foreground">
        <p>
          <span className="text-foreground">{session.stepper}</span> · {TYPE_LABEL[runTypeOfKind(session.kind)]} · Path
          ({start[0]}, {start[1]}) to ({end[0]}, {end[1]})
        </p>
        <p>
          Started with <span className="font-mono text-foreground">Velocity P {gains.velocity_p}</span> ·{" "}
          <span className="font-mono text-foreground">Velocity I {gains.velocity_i}</span> ·{" "}
          <span className="font-mono text-foreground">Position P {gains.position_p}</span>
        </p>
        {notes.map((line) => (
          <p key={line} className="text-amber-400">
            {line}
          </p>
        ))}
      </div>
    </Card>
  );
}
