import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  getTune,
  postTune,
  putTuneSetup,
  type Pattern,
  type PrinterView,
  type StepperView,
  type Tune,
  type TuneSetup,
} from "@/api";
import { ArrowRightIcon } from "lucide-react";
import { Button as UiButton } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { InputGroup, InputGroupAddon, InputGroupText } from "@/components/ui/input-group";
import { Switch } from "@/components/ui/switch";
import { failureMessage } from "@/failure";
import { defaultProbeSpeed, failureText, gatesBody, type GatesForm } from "@/guided";
import {
  bedLimits,
  browserStorage,
  DEFAULT_PARAMETERS,
  fallbackPattern,
  loadPattern,
  parseNumberList,
  patternIsolation,
  patternMode,
  patternProblem,
  probeEdgeHint,
  suggestedProbeStrokes,
  probeToggleSpacing,
  savePattern,
  toggleSpacingProblem,
  type PatternMode,
  type Setup,
} from "@/tune";
import { ProbeNotes } from "./EdgeHint";
import { ParametersBlock } from "./ParametersBlock";
import { Card, NumberField } from "./ui";
import { ActionBar, PatternCard } from "./WorkbenchCards";

const DEFAULT_GATES: GatesForm = { damping_floor: 0.02, marginal: 0.025, standstill_gate: false, standstill_factor: 2 };

/** Saving on a tune that has no step yet replaces its setup. Nothing moves until a step is confirmed. */
export function GuidedSetup({
  printer,
  tune,
  readOnly = false,
  offline = false,
  onCreated,
  onSaved,
  actions,
}: {
  printer: PrinterView;
  tune?: Tune;
  readOnly?: boolean;
  /** Set while foci-tune cannot be reached, so nothing is sent. */
  offline?: boolean;
  onCreated?: (tune: Tune) => void;
  onSaved?: (tune: Tune) => void;
  actions?: ReactNode;
}) {
  const recorded = tune?.setup;
  const storage = useMemo(() => browserStorage(), []);
  const limits = useMemo(() => bedLimits(printer.steppers), [printer.steppers]);
  const [stepperName, setStepperName] = useState(
    () => recorded?.stepper ?? (printer.steppers.find((s) => s.unusable === null) ?? printer.steppers[0]).name,
  );
  const stepper: StepperView | undefined = printer.steppers.find((s) => s.name === stepperName);
  const [speedsText, setSpeedsText] = useState(
    (recorded?.speeds_mm_s ?? DEFAULT_PARAMETERS.tracking.speeds_mm_s).join(", "),
  );
  const [typedPattern, setTypedPattern] = useState<Record<string, Pattern>>(() =>
    recorded === undefined ? {} : { [recorded.stepper]: recorded.pattern },
  );
  const [trackingStrokes, setTrackingStrokes] = useState<number>(
    recorded?.tracking_strokes ?? DEFAULT_PARAMETERS.tracking.strokes,
  );
  const [probe, setProbe] = useState(recorded?.probe ?? DEFAULT_PARAMETERS.probe);
  const [probeSpeedTouched, setProbeSpeedTouched] = useState(recorded !== undefined);
  const [probeStrokesTouched, setProbeStrokesTouched] = useState(recorded !== undefined);
  const [standstill, setStandstill] = useState(recorded?.standstill ?? DEFAULT_PARAMETERS.standstill);
  const [gates, setGates] = useState<GatesForm>(() =>
    recorded === undefined
      ? DEFAULT_GATES
      : {
          damping_floor: recorded.gates.damping_floor,
          marginal: recorded.gates.marginal,
          standstill_gate: recorded.gates.standstill_factor !== null,
          standstill_factor: recorded.gates.standstill_factor ?? DEFAULT_GATES.standstill_factor,
        },
  );
  const [includeMarginal, setIncludeMarginal] = useState(recorded?.include_marginal ?? false);
  const [busy, setBusy] = useState(false);
  const frozen = busy || readOnly;
  const [error, setError] = useState<string | null>(null);

  const [patternModes, setPatternModes] = useState<Record<string, PatternMode>>({});
  const rememberedFor = (s: StepperView): Pattern | null => {
    const typed = typedPattern[s.name];
    if (typed !== undefined) return typed;
    const remembered = loadPattern(storage, s.name);
    return remembered !== null && limits !== null && patternProblem(remembered, limits) === null ? remembered : null;
  };
  const mode: PatternMode =
    stepper === undefined
      ? "custom"
      : patternMode(patternModes[stepper.name], stepper.proposal.pattern, rememberedFor(stepper));
  const pattern: Pattern | null = (() => {
    if (stepper === undefined || limits === null) return null;
    if (mode === "default" && stepper.proposal.pattern !== null) return stepper.proposal.pattern;
    return rememberedFor(stepper) ?? stepper.proposal.pattern ?? fallbackPattern(limits);
  })();
  const patternError = pattern !== null && limits !== null ? patternProblem(pattern, limits) : null;
  const setPattern = (p: Pattern) => {
    if (stepper === undefined) return;
    setTypedPattern((prev) => ({ ...prev, [stepper.name]: p }));
    if (limits !== null && patternProblem(p, limits) === null) savePattern(storage, stepper.name, p);
  };
  const chooseMode = (next: PatternMode) => {
    if (stepper === undefined) return;
    setPatternModes((prev) => ({ ...prev, [stepper.name]: next }));
    if (next === "default" && stepper.proposal.pattern !== null) setPattern(stepper.proposal.pattern);
    else if (pattern !== null) setPattern(pattern);
  };
  const isolation =
    stepper === undefined || pattern === null
      ? null
      : patternIsolation({
          pattern,
          stepper,
          others: printer.steppers
            .filter((o) => o.serial === stepper.serial && o.name !== stepper.name)
            .map((o) => o.name),
          limits,
        });

  const speeds = parseNumberList(speedsText, "the speed set");
  const maxAccel = printer.moonraker.max_accel;
  // biome-ignore lint/correctness/useExhaustiveDependencies: the dependencies are the parsed parts of the inputs, so the default recomputes on a changed value and not on a new object.
  const derived = useMemo(
    () =>
      speeds.ok && pattern !== null && stepper !== undefined
        ? defaultProbeSpeed(speeds.values, {
            pattern,
            kinematics: stepper.kinematics,
            segmentsBetweenToggles: probe.segments_between_toggles,
            strokes: probeStrokesTouched ? probe.strokes : DEFAULT_PARAMETERS.probe.strokes,
            maxAccel,
          })
        : null,
    [
      speedsText,
      pattern?.start[0],
      pattern?.start[1],
      pattern?.end[0],
      pattern?.end[1],
      stepper?.kinematics,
      probe.segments_between_toggles,
      probeStrokesTouched,
      probe.strokes,
      maxAccel,
    ],
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
        })
      : null;
  useEffect(() => {
    if (!probeStrokesTouched && suggestedStrokes !== null) {
      setProbe((p) => (p.strokes === suggestedStrokes ? p : { ...p, strokes: suggestedStrokes }));
    }
  }, [suggestedStrokes, probeStrokesTouched]);

  const spacing =
    pattern !== null && stepper !== undefined
      ? probeToggleSpacing(pattern, stepper.kinematics, probe.speed_mm_s, probe.segments_between_toggles)
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
        })
      : null;

  const blocked: string[] = [];
  if (offline) blocked.push("Reconnecting to foci-tune.");
  if (stepper?.unusable) blocked.push(stepper.unusable);
  const precondition = printer.preconditions.find((p) => p.stepper === stepperName);
  if (precondition !== undefined && !precondition.ok) blocked.push(...precondition.failing);
  if (limits === null) blocked.push("bed limits need stepper_x and stepper_y in the config");
  if (!speeds.ok) blocked.push(speeds.error);
  else if (speeds.values.length < 1 || speeds.values.length > 3)
    blocked.push("a tune needs one to three tracking speeds");
  if (gates.damping_floor >= gates.marginal) blocked.push("the damping stop limit must be below the low-damping limit");

  const heldByPattern = patternError !== null || spacingProblem !== null || isolation?.ok === false;

  const submit = () => {
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
    const sent =
      tune === undefined
        ? postTune(body).then((t) => onCreated?.(t))
        : putTuneSetup(tune.id, body).then((t) => onSaved?.(t));
    sent
      .catch((e: unknown) =>
        tune === undefined
          ? setError(failureMessage(e))
          : getTune(tune.id).then(
              (fresh) => setError(failureText("setup", e, tune, fresh)),
              () => setError(failureText("setup", e, tune, null)),
            ),
      )
      .finally(() => setBusy(false));
  };

  const asSetup = (parameters: Setup["parameters"], type: Setup["type"]): Setup | null =>
    pattern === null || stepper === undefined ? null : ({ stepper: stepper.name, pattern, type, parameters } as Setup);
  const probeSetup = asSetup(probe, "probe");
  const trackingSetup = asSetup({ speeds_mm_s: speeds.ok ? speeds.values : [], strokes: trackingStrokes }, "tracking");
  const standstillSetup = asSetup(standstill, "standstill");

  const gateField = (label: string, key: "damping_floor" | "marginal", step: number) => (
    <label className="flex flex-col text-xs text-muted-foreground">
      {label}
      <NumberField
        label={label}
        value={gates[key]}
        step={step}
        disabled={frozen}
        onValue={(v) => setGates((g) => ({ ...g, [key]: v }))}
      />
    </label>
  );
  const toggle = (label: string, checked: boolean, onChange: (on: boolean) => void) => (
    <label className="flex items-center gap-2 text-sm">
      <Switch aria-label={label} checked={checked} disabled={frozen} onCheckedChange={onChange} />
      {label}
    </label>
  );

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card label="Tune setup">
        <div className="space-y-3">
          <label className="flex flex-col gap-1 text-xs text-muted-foreground">
            Stepper
            <Select value={stepperName} disabled={frozen || tune !== undefined} onValueChange={setStepperName}>
              <SelectTrigger aria-label="Stepper" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {printer.steppers.map((st) => (
                  <SelectItem key={st.name} value={st.name}>
                    {st.name}
                    {st.unusable !== null ? " (unusable)" : ""}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </label>
          <p className="text-sm font-bold text-muted-foreground">Tracking</p>
          {trackingSetup !== null && (
            <ParametersBlock
              type="tracking"
              setup={trackingSetup}
              speedsText={speedsText}
              disabled={frozen}
              onSpeedsText={setSpeedsText}
              onSetup={(s) => s.type === "tracking" && setTrackingStrokes(s.parameters.strokes)}
            />
          )}
          <p className="text-sm font-bold text-muted-foreground">Probe</p>
          {probeSetup !== null && (
            <ParametersBlock
              type="probe"
              setup={probeSetup}
              speedsText={speedsText}
              disabled={frozen}
              onSpeedsText={setSpeedsText}
              onSetup={(s) => {
                if (s.type !== "probe") return;
                if (s.parameters.speed_mm_s !== probe.speed_mm_s) setProbeSpeedTouched(true);
                if (s.parameters.strokes !== probe.strokes) setProbeStrokesTouched(true);
                setProbe(s.parameters);
              }}
            />
          )}
          {derived?.warning != null && !probeSpeedTouched && (
            <p role="note" className="rounded border border-amber-500/60 px-2 py-1 text-xs text-amber-300">
              {derived.warning}
            </p>
          )}
          <ProbeNotes spacingProblem={spacingProblem} hint={edgeHint} />
          <p className="text-sm font-bold text-muted-foreground">Standstill</p>
          {standstillSetup !== null && (
            <ParametersBlock
              type="standstill"
              setup={standstillSetup}
              speedsText={speedsText}
              disabled={frozen}
              onSpeedsText={setSpeedsText}
              onSetup={(s) => s.type === "standstill" && setStandstill(s.parameters)}
            />
          )}
          <p className="text-sm font-bold text-muted-foreground">Limits</p>
          <div className="flex flex-wrap items-end gap-3">
            {gateField("Damping stop limit", "damping_floor", 0.01)}
            {gateField("Low damping below", "marginal", 0.01)}
            <div className="flex flex-col text-xs text-muted-foreground">
              Noise limit
              <InputGroup className="w-48">
                <InputGroupAddon align="inline-start">
                  <Switch
                    aria-label="Check noise at standstill"
                    checked={gates.standstill_gate}
                    disabled={frozen}
                    onCheckedChange={(on) => setGates((g) => ({ ...g, standstill_gate: on }))}
                  />
                </InputGroupAddon>
                <NumberField
                  inGroup
                  label="Noise limit (times the baseline)"
                  value={gates.standstill_factor}
                  step={0.5}
                  disabled={frozen || !gates.standstill_gate}
                  onValue={(v) => setGates((g) => ({ ...g, standstill_factor: v }))}
                />
                <InputGroupAddon align="inline-end">
                  <InputGroupText>x baseline</InputGroupText>
                </InputGroupAddon>
              </InputGroup>
            </div>
          </div>
          <div className="space-y-1">
            {toggle("Include low-damping gains", includeMarginal, setIncludeMarginal)}
            <p className="text-xs text-muted-foreground">
              Gains whose damping is above the stop limit but below the low-damping limit are never recommended unless
              this is on. They tend to score best because they sit close to instability, so leave it off unless you want
              them considered.
            </p>
          </div>
        </div>
      </Card>
      <PatternCard
        limits={limits}
        pattern={pattern}
        proposal={stepper?.proposal.pattern ?? null}
        mode={mode}
        isolation={isolation === null || isolation.ok ? null : isolation}
        refusal={stepper?.proposal.refusal ?? null}
        error={patternError}
        notes={[]}
        disabled={frozen}
        onPattern={setPattern}
        onMode={chooseMode}
      />
      {!readOnly && (
        <div className="lg:col-span-2">
          <ActionBar blocked={[...new Set(blocked)]} message={error === null ? null : { tone: "error", text: error }}>
            {actions}
            <UiButton disabled={busy || blocked.length > 0 || heldByPattern || stepper === undefined} onClick={submit}>
              Next
              <ArrowRightIcon />
            </UiButton>
          </ActionBar>
        </div>
      )}
    </div>
  );
}
