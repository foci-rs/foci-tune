import { RotateCcwIcon } from "lucide-react";
import type { SessionView } from "@/api";
import { Button } from "@/components/ui/button";
import { runTypeOfKind } from "@/tune";
import { Card } from "./ui";

const TYPE_LABEL = { tracking: "Tracking", probe: "Probe", standstill: "Standstill" } as const;

export function SessionCard({
  view,
  canRestore,
  onRestore,
  notes,
}: {
  view: SessionView;
  canRestore: boolean;
  onRestore: () => void;
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
