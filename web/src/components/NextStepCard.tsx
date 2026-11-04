import { RotateCcwIcon } from "lucide-react";
import type { TuneAxis } from "@/api";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupText } from "@/components/ui/input-group";
import { Separator } from "@/components/ui/separator";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  axisLabel,
  STEP_DIVISIONS,
  stepPercent,
  type LadderPreview,
  type PrepareForm,
  type PrepareState,
} from "@/guided";
import { Card, NumberField } from "./ui";

const STEPS = Array.from({ length: 19 }, (_, i) => i - 9);

function StepSelect({
  label,
  value,
  disabled,
  onValue,
}: {
  label: string;
  value: number;
  disabled: boolean;
  onValue: (v: number) => void;
}) {
  return (
    <div className="flex flex-col text-xs text-muted-foreground">
      {label}
      <Select value={String(value)} disabled={disabled} onValueChange={(v) => onValue(Number(v))}>
        <SelectTrigger aria-label={label} className="w-24 font-mono text-foreground">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {STEPS.map((step) => (
            <SelectItem key={step} value={String(step)} className="font-mono">
              {step}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

const shownLive = (live: number, corner: boolean) => (corner ? `${live.toFixed(1)} Hz` : String(Math.round(live)));

const change = (step: number, divisions: number) => {
  const percent = Math.round((2 ** (step / divisions) - 1) * 100);
  return percent > 0 ? `+${percent}%` : `${percent}%`;
};

/** The ladder's steps from low to high: the step, the gain it gives and how far that is from the seed. */
function LadderTable({ preview, corner, divisions }: { preview: LadderPreview; corner: boolean; divisions: number }) {
  const columns = [
    ...preview.gains.map((g) => ({
      step: g.step,
      gain: corner ? `${g.shown.toFixed(1)} Hz` : String(g.raw),
      left: false,
    })),
    ...preview.left.map((l) => ({ step: l.step, gain: "left out", left: true })),
  ].sort((a, b) => a.step - b.step);
  const cell = (c: (typeof columns)[number]) =>
    `px-3 py-1 text-right font-mono ${c.left ? "text-muted-foreground" : ""} ${c.step === 0 ? "bg-muted/50 font-semibold" : ""}`;
  return (
    <div data-slot="ladder" className="mt-4 space-y-3">
      <Separator />
      <p className="text-sm font-bold text-muted-foreground">Sweep</p>
      <div className="overflow-x-auto">
        <table aria-label="Ladder" className="text-sm">
          <tbody>
            <tr>
              <th scope="row" className="py-1 pr-3 text-left text-xs font-normal text-muted-foreground">
                Step
              </th>
              {columns.map((c) => (
                <td key={c.step} data-seed={c.step === 0} className={`${cell(c)} text-xs text-muted-foreground`}>
                  {c.step}
                </td>
              ))}
            </tr>
            <tr>
              <th scope="row" className="py-1 pr-3 text-left text-xs font-normal text-muted-foreground">
                Gain
              </th>
              {columns.map((c) => (
                <td key={c.step} className={cell(c)}>
                  {c.gain}
                </td>
              ))}
            </tr>
            <tr>
              <th scope="row" className="py-1 pr-3 text-left text-xs font-normal text-muted-foreground">
                vs seed
              </th>
              {columns.map((c) => (
                <td key={c.step} className={`${cell(c)} text-xs text-muted-foreground`}>
                  {change(c.step, divisions)}
                </td>
              ))}
            </tr>
          </tbody>
        </table>
      </div>
      {preview.left.map((l) => (
        <p key={l.step} className="text-xs text-muted-foreground">
          Step {l.step} is left out because it {l.reason}.
        </p>
      ))}
    </div>
  );
}

/** The seed, the range of steps and the repeats of the next ladder, with the gains they give. */
export function NextStepCard({
  axis,
  busy,
  form,
  state,
  live,
  onForm,
}: {
  axis: TuneAxis;
  busy: boolean;
  form: PrepareForm;
  state: PrepareState;
  /** The stepper's live value on this axis, to go back to; null when it reports none. */
  live: number | null;
  onForm: (form: PrepareForm) => void;
}) {
  const corner = axis === "VelocityICornerHz";
  const set = (patch: Partial<PrepareForm>) => onForm({ ...form, ...patch });
  const seedShown = state.seed === null ? 0 : form.seed === null && corner ? Number(state.seed.toFixed(1)) : state.seed;
  return (
    <Card label={`Next step: ${axisLabel(axis)}`}>
      <div className="flex flex-wrap items-end gap-3">
        <div className="flex flex-col text-xs text-muted-foreground">
          Seed
          <InputGroup className="w-48">
            <NumberField inGroup label="Seed" value={seedShown} disabled={busy} onValue={(seed) => set({ seed })} />
            <InputGroupAddon align="inline-end">
              {state.unit !== "" && <InputGroupText>{state.unit}</InputGroupText>}
              <Tooltip>
                <TooltipTrigger asChild>
                  <span tabIndex={-1}>
                    <InputGroupButton
                      aria-label="Use the live value"
                      disabled={busy || live === null || form.seed === null}
                      onClick={() => set({ seed: null })}
                    >
                      <RotateCcwIcon />
                    </InputGroupButton>
                  </span>
                </TooltipTrigger>
                <TooltipContent>
                  {live === null ? "No live value is available" : `Use the live value: ${shownLive(live, corner)}`}
                </TooltipContent>
              </Tooltip>
            </InputGroupAddon>
          </InputGroup>
        </div>
        <label className="flex flex-col text-xs text-muted-foreground">
          Repeats
          <NumberField label="Repeats" value={form.repeats} disabled={busy} onValue={(repeats) => set({ repeats })} />
        </label>
        <div className="flex flex-col text-xs text-muted-foreground">
          Step size
          <Select value={String(form.divisions)} disabled={busy} onValueChange={(v) => set({ divisions: Number(v) })}>
            <SelectTrigger aria-label="Step size" className="w-32 font-mono text-foreground">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {STEP_DIVISIONS.map((d) => (
                <SelectItem key={d} value={String(d)} className="font-mono">
                  {stepPercent(d)}% (1/{d})
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <StepSelect label="Lowest step" value={form.lower} disabled={busy} onValue={(lower) => set({ lower })} />
        <StepSelect label="Highest step" value={form.upper} disabled={busy} onValue={(upper) => set({ upper })} />
      </div>
      {state.preview !== null && <LadderTable preview={state.preview} corner={corner} divisions={form.divisions} />}
    </Card>
  );
}
