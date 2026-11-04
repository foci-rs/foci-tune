import { useId } from "react";
import type { ControlSnapshot, Filters, LadderAxis } from "@/api";
import { formatNumber, PID_GAIN_MAX_RAW, type LadderFieldErrors, type LadderInput, type OuterGains } from "@/tune";
import { cornerHz, velocityIRaw } from "@/units";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { MinusIcon, PlusIcon, RefreshCwIcon, WandSparklesIcon } from "lucide-react";
import { Button as UiButton } from "@/components/ui/button";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  InputGroupText,
} from "@/components/ui/input-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Button, Card, NumberField } from "./ui";

export const GAIN_STEPS = { velocity_p: 50, position_p: 10, corner_hz: 5, filter_hz: 50 } as const;

const FILTERS: { key: keyof Filters; label: string }[] = [
  { key: "torque_hz", label: "Torque filter cutoff" },
  { key: "flux_hz", label: "Flux filter cutoff" },
  { key: "velocity_hz", label: "Velocity filter cutoff" },
  { key: "position_hz", label: "Position filter cutoff" },
];

export const LADDER_AXES: { axis: LadderAxis; label: string; unit: string }[] = [
  { axis: "velocity_p", label: "Velocity P", unit: "raw" },
  { axis: "velocity_i_corner_hz", label: "Velocity I", unit: "Hz" },
  { axis: "position_p", label: "Position P", unit: "raw" },
];

export function ladderAxisLabel(axis: string): string {
  const a = LADDER_AXES.find((x) => x.axis === axis);
  return a === undefined ? axis : `${a.label} (${a.unit})`;
}

/** Why the block's values cannot be written; empty when they can. */
export function gainProblems(gains: OuterGains, filters: Filters): string[] {
  const problems: string[] = [];
  const gain = (name: string, v: number) => {
    if (!Number.isInteger(v) || v < 0 || v > PID_GAIN_MAX_RAW) {
      problems.push(`${name} ${v} is outside 0..=${PID_GAIN_MAX_RAW}`);
    }
  };
  gain("velocity P", gains.velocity_p);
  gain("velocity I", gains.velocity_i);
  gain("position P", gains.position_p);
  for (const f of FILTERS) {
    const v = filters[f.key];
    if (!Number.isInteger(v) || v < 0 || v > 65535) {
      problems.push(`${f.label.toLowerCase()} ${v} Hz is not a whole number in 0..=65535`);
    }
  }
  return problems;
}

function Stepper({
  label,
  value,
  step,
  unit,
  disabled,
  onValue,
  hint,
  locked = false,
}: {
  label: string;
  value: number;
  step: number;
  unit: string;
  disabled: boolean;
  onValue: (v: number) => void;
  /** Explanation shown on hover or focus. */
  hint?: string;
  /** Shown like the others but not editable here. */
  locked?: boolean;
}) {
  const off = disabled || locked;
  const row = (
    <div className="flex items-center gap-1">
      <UiButton
        variant="outline"
        size="icon"
        aria-label={locked ? `Decrease ${label}` : `Decrease ${label} by ${step} ${unit}`}
        disabled={off}
        onClick={() => onValue(value - step)}
      >
        <MinusIcon />
      </UiButton>
      <NumberField label={`${label} (${unit})`} value={value} disabled={disabled} readOnly={locked} onValue={onValue} />
      <UiButton
        variant="outline"
        size="icon"
        aria-label={locked ? `Increase ${label}` : `Increase ${label} by ${step} ${unit}`}
        disabled={off}
        onClick={() => onValue(value + step)}
      >
        <PlusIcon />
      </UiButton>
      <span className="text-xs text-muted-foreground">{unit}</span>
    </div>
  );
  return (
    <div className="min-w-0">
      <p className="text-[0.65rem] uppercase tracking-wider text-muted-foreground">{label}</p>
      {hint === undefined ? (
        row
      ) : (
        <Tooltip>
          <TooltipTrigger asChild>{row}</TooltipTrigger>
          <TooltipContent side="bottom" align="start">
            {hint}
          </TooltipContent>
        </Tooltip>
      )}
    </div>
  );
}

function HeadingHint({ hint, last = false, children }: { hint: string; last?: boolean; children: string }) {
  return (
    <th className={last ? "py-1 text-left font-normal" : "py-1 pr-4 text-left font-normal"}>
      <Tooltip>
        <TooltipTrigger asChild>
          <span tabIndex={0}>{children}</span>
        </TooltipTrigger>
        <TooltipContent side="bottom" align="start">
          {hint}
        </TooltipContent>
      </Tooltip>
    </th>
  );
}

/** A refresh icon that loads the board's current values into a card. */
function LoadLiveButton({ label, disabled, onClick }: { label: string; disabled: boolean; onClick: () => void }) {
  return (
    <UiButton variant="outline" size="icon-sm" aria-label={label} disabled={disabled} onClick={onClick}>
      <RefreshCwIcon />
    </UiButton>
  );
}

function ReadOnlyRow({ label, p, i }: { label: string; p: number | undefined; i: number | undefined }) {
  return (
    <tr className="align-top">
      <th scope="row" className="py-2 pr-4 text-left font-normal text-muted-foreground">
        {label}
      </th>
      <td className="py-2 pr-4 font-mono text-sm">{p ?? "-"}</td>
      <td className="py-2 font-mono text-sm">{i ?? "-"}</td>
    </tr>
  );
}

/**
 * The gains written before every run. Torque and flux are the board's live values and cannot
 * be set from here; velocity P, velocity I as its corner with raw beside it and position P are
 * editable, and position I is read-only. With the ladder on, the ladder axis' field becomes the
 * value list.
 */
export function GainsCard({
  gains,
  positionI,
  live,
  pwmHz,
  disabled,
  sweptAxis,
  canLoadLive,
  onGains,
  onLoadLive,
}: {
  gains: OuterGains | null;
  positionI: number;
  live: ControlSnapshot | undefined;
  pwmHz: number | null;
  disabled: boolean;
  /** The gain the sweep sets, locked here; null while the sweep is off. */
  sweptAxis: LadderAxis | null;
  canLoadLive: boolean;
  onGains: (g: OuterGains) => void;
  onLoadLive: () => void;
}) {
  const action = <LoadLiveButton label="Load live gains" disabled={disabled || !canLoadLive} onClick={onLoadLive} />;
  if (gains === null) {
    return (
      <Card label="Gains" action={action}>
        <p className="text-sm text-muted-foreground">Waiting for a control-state frame from the channel.</p>
      </Card>
    );
  }
  const swept = (axis: LadderAxis) => sweptAxis === axis;
  const corner = pwmHz === null ? null : cornerHz(gains.velocity_i, pwmHz);
  return (
    <Card label="Gains" action={action}>
      <table aria-label="Gains" className="w-full text-sm">
        <thead className="border-b text-[0.65rem] uppercase tracking-wider text-muted-foreground">
          <tr>
            <th className="py-1 pr-4 text-left font-normal">Gains</th>
            <HeadingHint hint="Reacts to the current error.">P</HeadingHint>
            <HeadingHint hint="Corrects slow drift." last>
              I
            </HeadingHint>
          </tr>
        </thead>
        <tbody>
          <ReadOnlyRow label="Torque" p={live?.torque_p} i={live?.torque_i} />
          <ReadOnlyRow label="Flux" p={live?.flux_p} i={live?.flux_i} />
          <tr className="align-top">
            <th scope="row" className="py-2 pr-4 text-left font-normal text-muted-foreground">
              Velocity
            </th>
            <td className="py-2 pr-4">
              <Stepper
                label="Velocity P"
                value={gains.velocity_p}
                step={GAIN_STEPS.velocity_p}
                unit="raw"
                disabled={disabled}
                locked={swept("velocity_p")}
                onValue={(v) => onGains({ ...gains, velocity_p: Math.round(v) })}
              />
            </td>
            <td className="py-2">
              {corner === null || pwmHz === null ? (
                <p className="text-amber-400">Velocity I cannot be shown as a corner: the PWM frequency is unknown.</p>
              ) : (
                <Stepper
                  label="Velocity I"
                  value={Math.round(corner * 100) / 100}
                  step={GAIN_STEPS.corner_hz}
                  unit="Hz"
                  disabled={disabled}
                  hint={`Raw value ${gains.velocity_i}, as written to the config`}
                  locked={swept("velocity_i_corner_hz")}
                  onValue={(v) => onGains({ ...gains, velocity_i: velocityIRaw(v, pwmHz) })}
                />
              )}
            </td>
          </tr>
          <tr className="align-top">
            <th scope="row" className="py-2 pr-4 text-left font-normal text-muted-foreground">
              Position
            </th>
            <td className="py-2 pr-4">
              <Stepper
                label="Position P"
                value={gains.position_p}
                step={GAIN_STEPS.position_p}
                unit="raw"
                disabled={disabled}
                locked={swept("position_p")}
                onValue={(v) => onGains({ ...gains, position_p: Math.round(v) })}
              />
            </td>
            <td className="py-2">
              <Stepper
                label="Position I"
                value={positionI}
                step={0}
                unit="raw"
                disabled={disabled}
                locked
                onValue={() => {}}
              />
            </td>
          </tr>
        </tbody>
      </table>
    </Card>
  );
}

/** The filters written before every run. */
export function FiltersCard({
  filters,
  disabled,
  canLoadLive,
  onFilters,
  onLoadLive,
}: {
  filters: Filters | null;
  disabled: boolean;
  canLoadLive: boolean;
  onFilters: (f: Filters) => void;
  onLoadLive: () => void;
}) {
  return (
    <Card
      label="Filters"
      action={<LoadLiveButton label="Load live filters" disabled={disabled || !canLoadLive} onClick={onLoadLive} />}
    >
      {filters === null ? (
        <p className="text-sm text-muted-foreground">Waiting for a control-state frame from the channel.</p>
      ) : (
        <div className="grid gap-3">
          {FILTERS.map((f) => (
            <Stepper
              key={f.key}
              label={f.label}
              value={filters[f.key]}
              step={GAIN_STEPS.filter_hz}
              unit="Hz"
              disabled={disabled}
              onValue={(v) => onFilters({ ...filters, [f.key]: Math.round(v) })}
            />
          ))}
        </div>
      )}
    </Card>
  );
}

function LabelHint({ hint, children }: { hint: string; children: string }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span tabIndex={0} className="w-fit">
          {children}
        </span>
      </TooltipTrigger>
      <TooltipContent side="bottom" align="start" className="max-w-xs">
        {hint}
      </TooltipContent>
    </Tooltip>
  );
}

function FieldError({ id, message }: { id: string; message: string | null }) {
  if (message === null) return null;
  return (
    <p id={id} role="alert" className="max-w-sm text-xs text-red-400">
      {message}
    </p>
  );
}

/** The sweep's gain and values, runs per value and stop-rule thresholds. */
export function LadderFields({
  ladder,
  defaultsFor,
  errors,
  probe,
  disabled,
  onLadder,
}: {
  ladder: LadderInput;
  /** The suggested values for a gain. */
  defaultsFor: (axis: LadderAxis) => number[];
  errors: LadderFieldErrors;
  probe: boolean;
  disabled: boolean;
  onLadder: (l: LadderInput) => void;
}) {
  const axis = LADDER_AXES.find((a) => a.axis === ladder.axis);
  const id = useId();
  const [valuesId, runsId, thresholdsId] = [`${id}-values`, `${id}-runs`, `${id}-thresholds`];
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-start gap-3">
        <div className="flex flex-col gap-1 text-xs text-muted-foreground">
          Gain to sweep
          <Select
            value={ladder.axis}
            disabled={disabled}
            onValueChange={(next) =>
              onLadder({ ...ladder, axis: next as LadderAxis, text: defaultsFor(next as LadderAxis).join(", ") })
            }
          >
            <SelectTrigger aria-label="Gain to sweep" className="text-foreground">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {LADDER_AXES.map((a) => (
                <SelectItem key={a.axis} value={a.axis}>
                  {a.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex w-[22rem] max-w-full flex-col gap-1 text-xs text-muted-foreground">
          Values
          <InputGroup className="w-full">
            <InputGroupAddon align="inline-start">
              <InputGroupButton
                aria-label="Fill in suggested values"
                disabled={disabled || defaultsFor(ladder.axis).length === 0}
                onClick={() => onLadder({ ...ladder, text: defaultsFor(ladder.axis).join(", ") })}
              >
                <WandSparklesIcon />
              </InputGroupButton>
            </InputGroupAddon>
            <InputGroupInput
              type="text"
              aria-label={`${axis?.label} sweep values`}
              aria-invalid={errors.values !== null || undefined}
              aria-describedby={errors.values === null ? undefined : valuesId}
              value={ladder.text}
              disabled={disabled}
              onChange={(e) => onLadder({ ...ladder, text: e.target.value })}
              placeholder="100, 120, 140"
              className="font-mono text-foreground"
            />
            <InputGroupAddon align="inline-end">
              <InputGroupText>{axis?.unit}</InputGroupText>
            </InputGroupAddon>
          </InputGroup>
          <FieldError id={valuesId} message={errors.values} />
        </div>
      </div>
      <div className="flex flex-wrap items-start gap-3">
        <label className="flex flex-col text-xs text-muted-foreground">
          Runs per value
          <NumberField
            label="Runs per value"
            value={ladder.repeats}
            disabled={disabled}
            invalid={errors.runs !== null}
            describedBy={errors.runs === null ? undefined : runsId}
            onValue={(v) => onLadder({ ...ladder, repeats: v })}
            className="w-16"
          />
          <FieldError id={runsId} message={errors.runs} />
        </label>
        {probe && (
          <>
            <label className="flex flex-col text-xs text-muted-foreground">
              <LabelHint hint="The sweep stops after a value whose median damping comes out below this number. Lower damping means the motion rings more.">
                Stop below damping
              </LabelHint>
              <NumberField
                label="Stop below damping"
                value={ladder.floor}
                step={0.01}
                disabled={disabled}
                invalid={errors.thresholds !== null}
                describedBy={errors.thresholds === null ? undefined : thresholdsId}
                onValue={(v) => onLadder({ ...ladder, floor: v })}
                className="w-20"
              />
            </label>
            <label className="flex flex-col text-xs text-muted-foreground">
              <LabelHint hint="A value whose median damping is below this number is marked as marginal in the results. The sweep carries on.">
                Mark as marginal below
              </LabelHint>
              <NumberField
                label="Mark as marginal below"
                value={ladder.marginal}
                step={0.01}
                disabled={disabled}
                invalid={errors.thresholds !== null}
                describedBy={errors.thresholds === null ? undefined : thresholdsId}
                onValue={(v) => onLadder({ ...ladder, marginal: v })}
                className="w-20"
              />
            </label>
            <FieldError id={thresholdsId} message={errors.thresholds} />
          </>
        )}
      </div>
    </div>
  );
}

/** The Run button, disabled with every reason listed while any reason holds. */
export function RunControls({
  label,
  blocked,
  busy,
  onRun,
}: {
  label: string;
  blocked: string[];
  busy: boolean;
  onRun: () => void;
}) {
  return (
    <div className="space-y-2">
      <Button
        disabled={blocked.length > 0 || busy}
        onClick={onRun}
        className="border-emerald-600 bg-emerald-700 text-white hover:bg-emerald-600"
      >
        {busy ? "Submitting" : label}
      </Button>
      {blocked.length > 0 && (
        <ul aria-label="Why Run is disabled" className="list-disc pl-5 text-sm text-amber-400">
          {blocked.map((reason) => (
            <li key={reason}>{reason}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function formatCorner(raw: number, pwmHz: number | null): string {
  return pwmHz === null ? `raw ${raw}` : `${formatNumber(cornerHz(raw, pwmHz), 1)} Hz`;
}
