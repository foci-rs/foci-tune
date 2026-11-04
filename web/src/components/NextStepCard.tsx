import { RotateCcwIcon } from "lucide-react";
import type { TuneAxis } from "@/api";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupText } from "@/components/ui/input-group";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { axisLabel, type PrepareForm, type PrepareState } from "@/guided";
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

const plural = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;

/** The seed, the range of steps and the repeats of the next ladder, with the gains they give. */
export function NextStepCard({
  axis,
  busy,
  form,
  state,
  canReload,
  onForm,
}: {
  axis: TuneAxis;
  busy: boolean;
  form: PrepareForm;
  state: PrepareState;
  /** Whether the stepper reports a live value to go back to. */
  canReload: boolean;
  onForm: (form: PrepareForm) => void;
}) {
  const corner = axis === "VelocityICornerHz";
  const set = (patch: Partial<PrepareForm>) => onForm({ ...form, ...patch });
  const seedShown = state.seed === null ? 0 : form.seed === null && corner ? Number(state.seed.toFixed(1)) : state.seed;
  const items =
    state.preview === null
      ? []
      : [
          ...state.preview.gains.map((g) => ({
            step: g.step,
            gain: corner ? `${g.shown.toFixed(1)} Hz` : String(g.raw),
            reason: null,
          })),
          ...state.preview.left.map((l) => ({ step: l.step, gain: null, reason: l.reason })),
        ].sort((a, b) => a.step - b.step);
  return (
    <Card label={`Next step: ${axisLabel(axis)}`}>
      <div className="flex flex-wrap items-end gap-3">
        <div className="flex flex-col text-xs text-muted-foreground">
          Seed
          <InputGroup className="w-48">
            <NumberField inGroup label="Seed" value={seedShown} disabled={busy} onValue={(seed) => set({ seed })} />
            <InputGroupAddon align="inline-end">
              {state.unit !== "" && <InputGroupText>{state.unit}</InputGroupText>}
              <InputGroupButton
                aria-label="Use the live value"
                disabled={busy || !canReload || form.seed === null}
                onClick={() => set({ seed: null })}
              >
                <RotateCcwIcon />
              </InputGroupButton>
            </InputGroupAddon>
          </InputGroup>
        </div>
        <label className="flex flex-col text-xs text-muted-foreground">
          Repeats
          <NumberField label="Repeats" value={form.repeats} disabled={busy} onValue={(repeats) => set({ repeats })} />
        </label>
        <StepSelect label="Lowest step" value={form.lower} disabled={busy} onValue={(lower) => set({ lower })} />
        <StepSelect label="Highest step" value={form.upper} disabled={busy} onValue={(upper) => set({ upper })} />
      </div>
      {state.preview !== null && (
        <div className="mt-3 space-y-1">
          <ul aria-label="Ladder" className="flex flex-wrap gap-2">
            {items.map((item) => (
              <li
                key={item.step}
                className={`rounded border px-2 py-1 font-mono text-xs ${item.reason === null ? "" : "text-muted-foreground"}`}
              >
                <span className="mr-2 text-muted-foreground">{item.step}</span>
                {item.reason === null ? (
                  <span>{item.gain}</span>
                ) : (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <span tabIndex={0} className="font-sans underline decoration-dotted">
                        left out
                      </span>
                    </TooltipTrigger>
                    <TooltipContent>{item.reason}</TooltipContent>
                  </Tooltip>
                )}
              </li>
            ))}
          </ul>
          <p className="text-xs text-muted-foreground">
            {plural(state.preview.gains.length, "gain")}, {plural(form.repeats, "repeat")} each
          </p>
        </div>
      )}
    </Card>
  );
}
