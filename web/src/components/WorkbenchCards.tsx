import type { LadderAxis, Pattern } from "@/api";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { PlayIcon, TriangleAlertIcon } from "lucide-react";
import { Button as UiButton } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { LadderFieldErrors, LadderInput, Limits, PatternMode } from "@/tune";
import { BedMap } from "./BedMap";
import { LadderFields } from "./GainsBlock";
import { Card } from "./ui";

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

const sentence = (reason: string): string => {
  const text = reason.trim();
  const capped = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?]$/.test(capped) ? capped : `${capped}.`;
};

/** Run and Cancel, with the first reason Run is disabled, a count of the rest and the status. */
export function RunBar({
  runLabel,
  blocked,
  busy,
  onRun,
  running,
  onAbort,
  message,
}: {
  runLabel: string;
  blocked: string[];
  busy: boolean;
  onRun: () => void;
  running: boolean;
  onAbort: () => void;
  message: { tone: "ok" | "error"; text: string } | null;
}) {
  const runButton = (
    <UiButton
      disabled={blocked.length > 0 || busy}
      onClick={onRun}
      className="border border-emerald-600 bg-emerald-700 text-white hover:bg-emerald-600"
    >
      <PlayIcon />
      {busy ? "Submitting" : runLabel}
    </UiButton>
  );
  return (
    <div className="sticky bottom-0 z-40 -mx-4 border-t bg-background px-4 py-3">
      <div className="flex items-center justify-between gap-4">
        <div className="min-h-9 min-w-0 flex-1 space-y-1 text-sm">
          {blocked.length > 0 && (
            <p className="flex items-center gap-2 text-amber-400">
              <TriangleAlertIcon className="size-4 shrink-0" />
              <span>{sentence(blocked[0])}</span>
              {blocked.length > 1 && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span tabIndex={0} className="shrink-0 underline decoration-dotted">
                      +{blocked.length - 1} more
                    </span>
                  </TooltipTrigger>
                  <TooltipContent side="top" align="start" className="max-w-sm">
                    {blocked.slice(1).map((reason) => (
                      <p key={reason}>{sentence(reason)}</p>
                    ))}
                  </TooltipContent>
                </Tooltip>
              )}
            </p>
          )}
          {message !== null && (
            <p role="status" className={message.tone === "ok" ? "text-emerald-400" : "text-red-400"}>
              {message.text}
            </p>
          )}
          {blocked.length > 0 && (
            <ul aria-label="Why Run is disabled" className="sr-only">
              {blocked.map((reason) => (
                <li key={reason}>{reason}</li>
              ))}
            </ul>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {running && (
            <UiButton variant="outline" onClick={onAbort}>
              Cancel queued runs
            </UiButton>
          )}
          {blocked.length > 0 ? (
            <Tooltip>
              <TooltipTrigger asChild>
                <span tabIndex={0}>{runButton}</span>
              </TooltipTrigger>
              <TooltipContent side="top" align="end" className="max-w-sm">
                {blocked.map((reason) => (
                  <p key={reason}>{sentence(reason)}</p>
                ))}
              </TooltipContent>
            </Tooltip>
          ) : (
            runButton
          )}
        </div>
      </div>
    </div>
  );
}
