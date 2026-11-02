import { useEffect, useMemo, useState } from "react";
import { ApiRequestError, postTune, type Pattern, type PrinterView, type StepperView, type Tune, type TuneSetup } from "@/api";
import { defaultProbeSpeed, gatesBody, type GatesForm } from "@/guided";
import {
  bedLimits,
  browserStorage,
  DEFAULT_PARAMETERS,
  fallbackPattern,
  loadPattern,
  parseNumberList,
  patternProblem,
  probeEdgeHint,
  suggestedProbeStrokes,
  probeToggleSpacing,
  savePattern,
  toggleSpacingProblem,
  type Setup,
} from "@/tune";
import { BedMap } from "./BedMap";
import { ParametersBlock } from "./ParametersBlock";
import { Button, Card, NumberField } from "./ui";

const HINT_CLASS = {
  ok: "text-xs text-muted-foreground",
  caution: "rounded border border-amber-500/60 px-2 py-1 text-xs text-amber-300",
  warning: "rounded border border-red-500/60 px-2 py-1 text-xs text-red-300",
} as const;

const DEFAULT_GATES: GatesForm = { damping_floor: 0.02, marginal: 0.025, standstill_gate: false, standstill_factor: 2 };
const DEFAULT_SPEEDS = [50, 195, 400];

function errorText(e: unknown): string {
  if (e instanceof ApiRequestError) return e.field === null ? e.message : `${e.field}: ${e.message}`;
  return e instanceof Error ? e.message : "request failed";
}

/** Creating a tune only records its setup; nothing moves until a step is confirmed. */
export function GuidedSetup({ printer, onCreated }: { printer: PrinterView; onCreated: (tune: Tune) => void }) {
  const storage = useMemo(() => browserStorage(), []);
  const limits = useMemo(() => bedLimits(printer.steppers), [printer.steppers]);
  const [stepperName, setStepperName] = useState(
    () => (printer.steppers.find((s) => s.unusable === null) ?? printer.steppers[0]).name,
  );
  const stepper: StepperView | undefined = printer.steppers.find((s) => s.name === stepperName);
  const [speedsText, setSpeedsText] = useState(DEFAULT_SPEEDS.join(", "));
  const [typedPattern, setTypedPattern] = useState<Record<string, Pattern>>({});
  const [trackingStrokes, setTrackingStrokes] = useState<number>(DEFAULT_PARAMETERS.tracking.strokes);
  const [probe, setProbe] = useState(DEFAULT_PARAMETERS.probe);
  const [probeSpeedTouched, setProbeSpeedTouched] = useState(false);
  const [probeStrokesTouched, setProbeStrokesTouched] = useState(false);
  const [standstill, setStandstill] = useState(DEFAULT_PARAMETERS.standstill);
  const [gates, setGates] = useState(DEFAULT_GATES);
  const [includeMarginal, setIncludeMarginal] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pattern: Pattern | null = (() => {
    if (stepper === undefined || limits === null) return null;
    const typed = typedPattern[stepper.name];
    if (typed !== undefined) return typed;
    const remembered = loadPattern(storage, stepper.name);
    if (remembered !== null && patternProblem(remembered, limits) === null) return remembered;
    return stepper.proposal.pattern ?? fallbackPattern(limits);
  })();
  const patternError = pattern !== null && limits !== null ? patternProblem(pattern, limits) : null;
  const setPattern = (p: Pattern) => {
    if (stepper === undefined) return;
    setTypedPattern((prev) => ({ ...prev, [stepper.name]: p }));
    if (limits !== null && patternProblem(p, limits) === null) savePattern(storage, stepper.name, p);
  };

  const speeds = parseNumberList(speedsText, "the speed set");
  const maxVelocity = printer.moonraker.max_velocity;
  const derived = useMemo(
    () =>
      speeds.ok && pattern !== null && stepper !== undefined
        ? defaultProbeSpeed(speeds.values, {
            pattern,
            kinematics: stepper.kinematics,
            segmentsBetweenToggles: probe.segments_between_toggles,
            strokes: probeStrokesTouched ? probe.strokes : DEFAULT_PARAMETERS.probe.strokes,
            maxVelocity,
          })
        : null,
    [speedsText, pattern?.start[0], pattern?.start[1], pattern?.end[0], pattern?.end[1], stepper?.kinematics, probe.segments_between_toggles, probeStrokesTouched, probe.strokes, maxVelocity],
  );
  useEffect(() => {
    if (!probeSpeedTouched && derived !== null && derived.speed > 0) {
      setProbe((p) => (p.speed_mm_s === derived.speed ? p : { ...p, speed_mm_s: derived.speed }));
    }
  }, [derived, probeSpeedTouched]);

  const suggestedStrokes =
    pattern !== null && stepper !== undefined
      ? suggestedProbeStrokes({
          pattern,
          kinematics: stepper.kinematics,
          speedMmS: probe.speed_mm_s,
          segmentsBetweenToggles: probe.segments_between_toggles,
          maxVelocity,
        })
      : null;
  useEffect(() => {
    if (!probeStrokesTouched && suggestedStrokes !== null) {
      setProbe((p) => (p.strokes === suggestedStrokes ? p : { ...p, strokes: suggestedStrokes }));
    }
  }, [suggestedStrokes, probeStrokesTouched]);

  const spacing =
    pattern !== null && stepper !== undefined
      ? probeToggleSpacing(pattern, stepper.kinematics, probe.speed_mm_s, probe.segments_between_toggles, maxVelocity)
      : null;
  const spacingProblem = spacing === null ? null : toggleSpacingProblem(spacing);
  const edgeHint =
    pattern !== null && stepper !== undefined
      ? probeEdgeHint({
          pattern,
          kinematics: stepper.kinematics,
          speedMmS: probe.speed_mm_s,
          segmentsBetweenToggles: probe.segments_between_toggles,
          strokes: probe.strokes,
          maxVelocity,
        })
      : null;

  const blocked: string[] = [];
  if (stepper?.unusable) blocked.push(stepper.unusable);
  const precondition = printer.preconditions.find((p) => p.stepper === stepperName);
  if (precondition !== undefined && !precondition.ok) blocked.push(...precondition.failing);
  if (limits === null) blocked.push("bed limits need stepper_x and stepper_y in the config");
  if (patternError !== null) blocked.push(patternError);
  if (!speeds.ok) blocked.push(speeds.error);
  else if (speeds.values.length < 1 || speeds.values.length > 3) blocked.push("a tune needs one to three tracking speeds");
  if (spacingProblem !== null) blocked.push(spacingProblem);
  if (gates.damping_floor >= gates.marginal) blocked.push("the damping floor must be below the marginal threshold");

  const create = () => {
    if (stepper === undefined || pattern === null || !speeds.ok) return;
    const body: TuneSetup = {
      stepper: stepper.name,
      speeds_mm_s: speeds.values,
      pattern,
      probe,
      tracking_strokes: trackingStrokes,
      standstill,
      gates: gatesBody(gates),
      include_marginal: includeMarginal,
    };
    setBusy(true);
    setError(null);
    postTune(body)
      .then(onCreated, (e: unknown) => setError(errorText(e)))
      .finally(() => setBusy(false));
  };

  const asSetup = (parameters: Setup["parameters"], type: Setup["type"]): Setup | null =>
    pattern === null || stepper === undefined
      ? null
      : ({ stepper: stepper.name, pattern, type, parameters } as Setup);
  const probeSetup = asSetup(probe, "probe");
  const trackingSetup = asSetup({ speeds_mm_s: speeds.ok ? speeds.values : [], strokes: trackingStrokes }, "tracking");
  const standstillSetup = asSetup(standstill, "standstill");

  const gateField = (label: string, key: "damping_floor" | "marginal" | "standstill_factor", step: number) => (
    <label className="flex flex-col text-xs text-muted-foreground">
      {label}
      <NumberField label={label} value={gates[key]} step={step}
        disabled={busy || (key === "standstill_factor" && !gates.standstill_gate)}
        onValue={(v) => setGates((g) => ({ ...g, [key]: v }))} />
    </label>
  );

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card label="Tune setup">
        <div className="space-y-3">
          <label className="flex flex-col text-xs text-muted-foreground">
            Stepper
            <select
              value={stepperName}
              disabled={busy}
              onChange={(e) => setStepperName(e.target.value)}
              className="w-fit rounded border bg-background px-1.5 py-1 text-sm text-foreground"
            >
              {printer.steppers.map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name}
                  {s.unusable !== null ? " (unusable)" : ""}
                </option>
              ))}
            </select>
          </label>
          <p className="text-xs text-muted-foreground">Tracking</p>
          {trackingSetup !== null && (
            <ParametersBlock
              type="tracking"
              setup={trackingSetup}
              speedsText={speedsText}
              disabled={busy}
              onSpeedsText={setSpeedsText}
              onSetup={(s) => s.type === "tracking" && setTrackingStrokes(s.parameters.strokes)}
            />
          )}
          <p className="text-xs text-muted-foreground">Probe</p>
          {probeSetup !== null && (
            <ParametersBlock
              type="probe"
              setup={probeSetup}
              speedsText={speedsText}
              disabled={busy}
              onSpeedsText={setSpeedsText}
              onSetup={(s) => {
                if (s.type !== "probe") return;
                if (s.parameters.speed_mm_s !== probe.speed_mm_s) setProbeSpeedTouched(true);
                if (s.parameters.strokes !== probe.strokes) setProbeStrokesTouched(true);
                setProbe(s.parameters);
              }}
            />
          )}
          {derived?.warning != null && !probeSpeedTouched && <p className="text-xs text-amber-400">{derived.warning}</p>}
          {spacingProblem !== null && <p className="text-xs text-amber-400">{spacingProblem}</p>}
          {edgeHint !== null && (
            <p role="note" className={HINT_CLASS[edgeHint.tone]}>
              {edgeHint.text}
            </p>
          )}
          <p className="text-xs text-muted-foreground">Standstill</p>
          {standstillSetup !== null && (
            <ParametersBlock
              type="standstill"
              setup={standstillSetup}
              speedsText={speedsText}
              disabled={busy}
              onSpeedsText={setSpeedsText}
              onSetup={(s) => s.type === "standstill" && setStandstill(s.parameters)}
            />
          )}
          <p className="text-xs text-muted-foreground">Gates</p>
          <div className="flex flex-wrap items-end gap-3">
            {gateField("Damping floor", "damping_floor", 0.01)}
            {gateField("Marginal below", "marginal", 0.01)}
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={gates.standstill_gate}
                disabled={busy}
                onChange={(e) => setGates((g) => ({ ...g, standstill_gate: e.target.checked }))}
              />
              Standstill gate
            </label>
            {gateField("Standstill factor", "standstill_factor", 0.5)}
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={includeMarginal}
                disabled={busy}
                onChange={(e) => setIncludeMarginal(e.target.checked)}
              />
              Include marginal rungs
            </label>
          </div>
        </div>
      </Card>
      <Card label="Pattern">
        {stepper?.proposal.refusal != null && (
          <p className="mb-2 text-xs text-amber-400">
            No proposed pattern: {stepper.proposal.refusal}. The pattern is yours to choose.
          </p>
        )}
        {limits !== null && pattern !== null && stepper !== undefined && (
          <div>
            <BedMap limits={limits} pattern={pattern} proposal={stepper.proposal.pattern} disabled={busy} onChange={setPattern} />
            {patternError !== null && (
              <p role="alert" className="mt-2 text-sm text-amber-400">
                Pattern refused: {patternError}
              </p>
            )}
            {stepper.proposal.pattern !== null && (
              <Button className="mt-2 text-xs" disabled={busy} onClick={() => setPattern(stepper.proposal.pattern as Pattern)}>
                Use the proposed pattern
              </Button>
            )}
          </div>
        )}
        <div className="mt-4 space-y-2 border-t pt-4">
          <p className="text-xs text-muted-foreground">
            Creating the tune records this setup and the starting gains. Nothing moves until you start a step.
          </p>
          {blocked.length > 0 && (
            <ul className="list-disc pl-5 text-xs text-amber-400">
              {[...new Set(blocked)].map((b) => (
                <li key={b}>{b}</li>
              ))}
            </ul>
          )}
          <Button disabled={busy || blocked.length > 0 || stepper === undefined} onClick={create}>
            Create tune
          </Button>
          {error !== null && (
            <p role="alert" className="text-sm text-red-400">
              {error}
            </p>
          )}
        </div>
      </Card>
    </div>
  );
}
