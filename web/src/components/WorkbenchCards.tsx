import type { LadderAxis, Pattern } from "@/api";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import type { LadderFieldErrors, LadderInput, Limits, PatternMode } from "@/tune";
import { BedMap } from "./BedMap";
import { LadderFields, RunControls } from "./GainsBlock";
import { Button, Card } from "./ui";

export interface PatternNote {
  tone: "ok" | "caution" | "warning";
  text: string;
}

const NOTE_CLASS: Record<PatternNote["tone"], string> = {
  ok: "text-xs text-muted-foreground",
  caution: "rounded border border-amber-500/60 px-2 py-1 text-xs text-amber-300",
  warning: "rounded border border-red-500/60 px-2 py-1 text-xs text-red-300",
};

/** The stroke pattern on the bed: the proposed one, locked, or a custom one to edit. */
export function PatternCard({
  limits,
  pattern,
  proposal,
  mode,
  refusal,
  error,
  notes,
  disabled,
  onPattern,
  onMode,
}: {
  limits: Limits | null;
  pattern: Pattern | null;
  proposal: Pattern | null;
  mode: PatternMode;
  refusal: string | null;
  error: string | null;
  notes: PatternNote[];
  disabled: boolean;
  onPattern: (p: Pattern) => void;
  onMode: (mode: PatternMode) => void;
}) {
  return (
    <Card label="Path">
      <div className="space-y-3">
        {limits !== null && pattern !== null && (
          <BedMap
            limits={limits}
            pattern={pattern}
            proposal={proposal}
            proposalTitle="Default path"
            disabled={disabled || mode === "default"}
            onChange={onPattern}
          >
            <Select value={mode} disabled={disabled} onValueChange={(v) => onMode(v as PatternMode)}>
              <SelectTrigger aria-label="Path" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="default" disabled={proposal === null}>
                  Default
                </SelectItem>
                <SelectItem value="custom">Custom</SelectItem>
              </SelectContent>
            </Select>
          </BedMap>
        )}
        {error !== null && (
          <p role="alert" className="text-sm text-amber-400">
            Path refused: {error}
          </p>
        )}
        {notes.map((n) => (
          <p key={n.text} role="note" className={NOTE_CLASS[n.tone]}>
            {n.text}
          </p>
        ))}
        {refusal !== null && (
          <p className="text-xs text-amber-400">No default path: {refusal}. The path is yours to choose.</p>
        )}
      </div>
    </Card>
  );
}

/** The ladder sweep: one line while off, its setup while on. */
export function LadderCard({
  on,
  ladder,
  defaultsFor,
  errors,
  probe,
  disabled,
  onToggle,
  onLadder,
}: {
  on: boolean;
  ladder: LadderInput;
  defaultsFor: (axis: LadderAxis) => number[];
  errors: LadderFieldErrors;
  probe: boolean;
  disabled: boolean;
  onToggle: (on: boolean) => void;
  onLadder: (l: LadderInput) => void;
}) {
  return (
    <Card
      label="Gain sweep"
      action={<Switch aria-label="Gain sweep" checked={on} disabled={disabled} onCheckedChange={onToggle} />}
    >
      {on && (
        <LadderFields
          ladder={ladder}
          defaultsFor={defaultsFor}
          errors={errors}
          probe={probe}
          disabled={disabled}
          onLadder={onLadder}
        />
      )}
    </Card>
  );
}

/** Run, Revert and Abort, with every reason Run is disabled and the session note. */
export function RunBar({
  runLabel,
  blocked,
  busy,
  onRun,
  canRevert,
  onRevert,
  running,
  onAbort,
  message,
  sessionNote,
}: {
  runLabel: string;
  blocked: string[];
  busy: boolean;
  onRun: () => void;
  canRevert: boolean;
  onRevert: () => void;
  running: boolean;
  onAbort: () => void;
  message: { tone: "ok" | "error"; text: string } | null;
  sessionNote: string;
}) {
  return (
    <div className="sticky bottom-0 z-40 -mx-4 border-t bg-background px-4 py-3">
      <div className="flex flex-wrap items-start gap-3">
        <RunControls label={runLabel} blocked={blocked} busy={busy} onRun={onRun} />
        <Button disabled={!canRevert} onClick={onRevert}>
          Restore starting values
        </Button>
        {running && <Button onClick={onAbort}>Cancel queued runs</Button>}
      </div>
      {message !== null && (
        <p
          role="status"
          className={message.tone === "ok" ? "mt-2 text-sm text-emerald-400" : "mt-2 text-sm text-red-400"}
        >
          {message.text}
        </p>
      )}
      <p className="mt-2 text-xs text-muted-foreground">{sessionNote}</p>
    </div>
  );
}
