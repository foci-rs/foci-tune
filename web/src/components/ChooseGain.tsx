import { ChevronDownIcon } from "lucide-react";
import { useState } from "react";
import type { TuneAxis } from "@/api";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { manualProblem } from "@/guided";
import { NumberField } from "./ui";

export function ChooseGain({
  axis,
  accept,
  initial,
  gains,
  pwmHz,
  busy,
  onAccept,
  onPick,
  onManual,
}: {
  axis: TuneAxis;
  /** The recommended gain as shown on the button; null when there is no recommendation. */
  accept: string | null;
  initial: number | null;
  gains: { index: number; label: string }[];
  pwmHz: number;
  busy: boolean;
  onAccept: () => void;
  onPick: (index: number) => void;
  onManual: (value: number) => void;
}) {
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState(initial ?? 0);
  const problem = manualProblem(axis, value, pwmHz);
  return (
    <>
      <div className="inline-flex rounded-md shadow-xs">
        <Tooltip>
          <TooltipTrigger asChild>
            <span tabIndex={-1} className="inline-flex">
              <Button className="rounded-r-none" disabled={busy || accept === null} onClick={onAccept}>
                Accept
              </Button>
            </span>
          </TooltipTrigger>
          <TooltipContent>
            {accept === null
              ? "No gain was scored, so there is nothing to accept."
              : `Take the recommended gain, ${accept}.`}
          </TooltipContent>
        </Tooltip>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label="More ways to choose"
              className="rounded-l-none border-l border-primary-foreground/30 px-2"
              disabled={busy}
            >
              <ChevronDownIcon />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {gains.map((g) => (
              <DropdownMenuItem key={g.index} onSelect={() => onPick(g.index)}>
                {`Rung ${g.index + 1}: ${g.label}`}
              </DropdownMenuItem>
            ))}
            {gains.length > 0 && <DropdownMenuSeparator />}
            <DropdownMenuItem
              onSelect={() => {
                setValue(initial ?? 0);
                setOpen(true);
              }}
            >
              Enter a value…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Enter a gain</DialogTitle>
            <DialogDescription>The gain to keep for this axis, instead of one that was measured.</DialogDescription>
          </DialogHeader>
          <label className="flex flex-col gap-1 text-xs text-muted-foreground">
            Gain
            <NumberField label="Gain" value={value} onValue={setValue} className="w-40" />
          </label>
          {problem !== null && <p className="text-sm text-amber-400">{problem}</p>}
          <DialogFooter>
            <Button variant="outline" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button
              disabled={problem !== null}
              onClick={() => {
                onManual(value);
                setOpen(false);
              }}
            >
              Confirm
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
