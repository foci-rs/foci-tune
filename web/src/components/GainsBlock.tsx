import type { Filters, LadderAxis } from "@/api";
import { formatNumber, PID_GAIN_MAX_RAW, type LadderInput, type OuterGains } from "@/tune";
import { cornerHz, velocityIRaw } from "@/units";
import { Button, NumberField } from "./ui";

export const GAIN_STEPS = { velocity_p: 50, position_p: 10, corner_hz: 5, filter_hz: 50 } as const;

const FILTERS: { key: keyof Filters; label: string }[] = [
  { key: "velocity_hz", label: "Velocity filter" },
  { key: "torque_hz", label: "Torque filter" },
  { key: "position_hz", label: "Position filter" },
  { key: "flux_hz", label: "Flux filter" },
];

export const LADDER_AXES: { axis: LadderAxis; label: string; unit: string }[] = [
  { axis: "velocity_p", label: "Velocity P", unit: "raw" },
  { axis: "velocity_i_corner_hz", label: "Velocity I corner", unit: "Hz" },
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
  aside,
}: {
  label: string;
  value: number;
  step: number;
  unit: string;
  disabled: boolean;
  onValue: (v: number) => void;
  aside?: string;
}) {
  return (
    <div className="min-w-0">
      <p className="text-[0.65rem] uppercase tracking-wider text-muted-foreground">{label}</p>
      <div className="flex items-center gap-1">
        <Button aria-label={`Decrease ${label} by ${step} ${unit}`} disabled={disabled} onClick={() => onValue(value - step)} className="px-2 py-0.5">
          -
        </Button>
        <NumberField label={`${label} (${unit})`} value={value} disabled={disabled} onValue={onValue} />
        <Button aria-label={`Increase ${label} by ${step} ${unit}`} disabled={disabled} onClick={() => onValue(value + step)} className="px-2 py-0.5">
          +
        </Button>
        <span className="text-xs text-muted-foreground">{unit}</span>
        {aside !== undefined && <span className="font-mono text-xs text-muted-foreground">{aside}</span>}
      </div>
    </div>
  );
}

function LadderValues({
  ladder,
  onLadder,
  disabled,
  defaults,
}: {
  ladder: LadderInput;
  onLadder: (l: LadderInput) => void;
  disabled: boolean;
  defaults: number[];
}) {
  const axis = LADDER_AXES.find((a) => a.axis === ladder.axis);
  return (
    <div className="min-w-0 rounded-md border border-emerald-700/60 p-2">
      <p className="text-[0.65rem] uppercase tracking-wider text-emerald-300">
        {axis?.label} ladder ({axis?.unit})
      </p>
      <div className="flex flex-wrap items-center gap-1">
        <input
          type="text"
          aria-label={`${axis?.label} ladder values`}
          value={ladder.text}
          disabled={disabled}
          onChange={(e) => onLadder({ ...ladder, text: e.target.value })}
          placeholder="100, 120, 140"
          className="w-56 rounded border bg-background px-1.5 py-0.5 font-mono text-sm"
        />
        <Button
          disabled={disabled || defaults.length === 0}
          onClick={() => onLadder({ ...ladder, text: defaults.join(", ") })}
          className="px-2 py-0.5 text-xs"
        >
          Defaults
        </Button>
      </div>
    </div>
  );
}

/**
 * The gains written before every run: velocity P, velocity I as its corner with raw beside it,
 * position P, the filters, and position I read-only. With the ladder on, the ladder axis'
 * field becomes the value list.
 */
export function GainsBlock({
  gains,
  filters,
  positionI,
  pwmHz,
  disabled,
  ladder,
  ladderOn,
  ladderDefaults,
  onGains,
  onFilters,
  onLadder,
}: {
  gains: OuterGains;
  filters: Filters;
  positionI: { value: number; source: string };
  pwmHz: number | null;
  disabled: boolean;
  ladder: LadderInput;
  ladderOn: boolean;
  ladderDefaults: number[];
  onGains: (g: OuterGains) => void;
  onFilters: (f: Filters) => void;
  onLadder: (l: LadderInput) => void;
}) {
  const laddered = (axis: LadderAxis) => ladderOn && ladder.axis === axis;
  const ladderField = (
    <LadderValues ladder={ladder} onLadder={onLadder} disabled={disabled} defaults={ladderDefaults} />
  );
  const corner = pwmHz === null ? null : cornerHz(gains.velocity_i, pwmHz);
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      {laddered("velocity_p") ? (
        ladderField
      ) : (
        <Stepper
          label="Velocity P"
          value={gains.velocity_p}
          step={GAIN_STEPS.velocity_p}
          unit="raw"
          disabled={disabled}
          onValue={(v) => onGains({ ...gains, velocity_p: Math.round(v) })}
        />
      )}
      {laddered("velocity_i_corner_hz") ? (
        ladderField
      ) : corner === null || pwmHz === null ? (
        <p className="text-sm text-amber-400">Velocity I cannot be shown as a corner: the PWM frequency is unknown.</p>
      ) : (
        <Stepper
          label="Velocity I corner"
          value={Math.round(corner * 100) / 100}
          step={GAIN_STEPS.corner_hz}
          unit="Hz"
          disabled={disabled}
          aside={`raw ${gains.velocity_i}`}
          onValue={(v) => onGains({ ...gains, velocity_i: velocityIRaw(v, pwmHz) })}
        />
      )}
      {laddered("position_p") ? (
        ladderField
      ) : (
        <Stepper
          label="Position P"
          value={gains.position_p}
          step={GAIN_STEPS.position_p}
          unit="raw"
          disabled={disabled}
          onValue={(v) => onGains({ ...gains, position_p: Math.round(v) })}
        />
      )}
      <div className="min-w-0">
        <p className="text-[0.65rem] uppercase tracking-wider text-muted-foreground">Position I</p>
        <p className="font-mono text-sm">
          {positionI.value} <span className="text-xs text-muted-foreground">raw, read-only, {positionI.source}</span>
        </p>
      </div>
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
  );
}

/** Ladder toggle and its axis, repeats and stop-rule thresholds. */
export function LadderSetup({
  on,
  ladder,
  probe,
  disabled,
  onToggle,
  onLadder,
}: {
  on: boolean;
  ladder: LadderInput;
  probe: boolean;
  disabled: boolean;
  onToggle: (on: boolean) => void;
  onLadder: (l: LadderInput) => void;
}) {
  return (
    <div className="flex flex-wrap items-end gap-3">
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={on} disabled={disabled} onChange={(e) => onToggle(e.target.checked)} />
        Ladder
      </label>
      {on && (
        <>
          <label className="flex flex-col text-xs text-muted-foreground">
            Axis
            <select
              value={ladder.axis}
              disabled={disabled}
              onChange={(e) => onLadder({ ...ladder, axis: e.target.value as LadderAxis, text: "" })}
              className="rounded border bg-background px-1.5 py-1 text-sm text-foreground"
            >
              {LADDER_AXES.map((a) => (
                <option key={a.axis} value={a.axis}>
                  {a.label} ({a.unit})
                </option>
              ))}
            </select>
          </label>
          <label className="flex flex-col text-xs text-muted-foreground">
            Repeats
            <NumberField label="Repeats" value={ladder.repeats} disabled={disabled} onValue={(v) => onLadder({ ...ladder, repeats: v })} className="w-16" />
          </label>
          <label className="flex flex-col text-xs text-muted-foreground">
            Zeta floor
            <NumberField label="Zeta floor" value={ladder.floor} step={0.01} disabled={disabled} onValue={(v) => onLadder({ ...ladder, floor: v })} className="w-20" />
          </label>
          <label className="flex flex-col text-xs text-muted-foreground">
            Marginal below
            <NumberField label="Marginal below" value={ladder.marginal} step={0.01} disabled={disabled} onValue={(v) => onLadder({ ...ladder, marginal: v })} className="w-20" />
          </label>
          {!probe && (
            <p className="text-xs text-muted-foreground">The stop rule applies to probe ladders only; this ladder runs to completion.</p>
          )}
        </>
      )}
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
        <ul aria-label="Why the run cannot start" className="list-disc pl-5 text-sm text-amber-400">
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
