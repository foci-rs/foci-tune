import { NumberField } from "@/components/ui";
import type { RunType, Setup } from "@/tune";

export function ParametersBlock({
  type,
  setup,
  speedsText,
  disabled,
  onSpeedsText,
  onSetup,
}: {
  type: RunType;
  setup: Setup;
  speedsText: string;
  disabled: boolean;
  onSpeedsText: (t: string) => void;
  onSetup: (s: Setup) => void;
}) {
  const field = (label: string, value: number, apply: (v: number) => Setup, step?: number) => (
    <label className="flex flex-col text-xs text-muted-foreground">
      {label}
      <NumberField label={label} value={value} step={step} disabled={disabled} onValue={(v) => onSetup(apply(v))} />
    </label>
  );
  if (setup.type === "tracking" && type === "tracking") {
    const p = setup.parameters;
    return (
      <div className="flex flex-wrap gap-3">
        <label className="flex flex-col text-xs text-muted-foreground">
          Speeds (mm/s)
          <input
            type="text"
            aria-label="Speeds (mm/s)"
            value={speedsText}
            disabled={disabled}
            onChange={(e) => onSpeedsText(e.target.value)}
            className="w-48 rounded border bg-background px-1.5 py-0.5 font-mono text-sm text-foreground"
          />
        </label>
        {field("Strokes per speed", p.strokes, (v) => ({ ...setup, parameters: { ...p, strokes: v } }))}
      </div>
    );
  }
  if (setup.type === "probe") {
    const p = setup.parameters;
    return (
      <div className="flex flex-wrap gap-3">
        {field("Speed (mm/s)", p.speed_mm_s, (v) => ({ ...setup, parameters: { ...p, speed_mm_s: v } }))}
        {field("High gain", p.high_gain, (v) => ({ ...setup, parameters: { ...p, high_gain: v } }), 0.05)}
        {field("Segments between toggles", p.segments_between_toggles, (v) => ({
          ...setup,
          parameters: { ...p, segments_between_toggles: v },
        }))}
        {field("Strokes", p.strokes, (v) => ({ ...setup, parameters: { ...p, strokes: v } }))}
      </div>
    );
  }
  if (setup.type === "standstill") {
    const p = setup.parameters;
    return (
      <div className="flex flex-wrap gap-3">
        {field("Rests", p.rests, (v) => ({ ...setup, parameters: { ...p, rests: v } }))}
        {field("Dwell (s)", p.dwell_s, (v) => ({ ...setup, parameters: { ...p, dwell_s: v } }), 0.5)}
      </div>
    );
  }
  return null;
}
