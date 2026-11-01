import { createRoute } from "@tanstack/react-router";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ApiRequestError,
  getConfigBlock,
  getSeries,
  getSession,
  getSessions,
  postAbort,
  postLadder,
  postRevert,
  postRun,
  type ChannelPlot,
  type Pattern,
  type PrinterView,
  type RunSetup,
  type Series,
  type SessionId,
  type SessionView,
  type StepperView,
} from "@/api";
import { ParametersBlock } from "@/components/ParametersBlock";
import { BedMap } from "@/components/BedMap";
import { EnsemblePlot } from "@/components/EnsemblePlot";
import { gainProblems, GainsBlock, ladderAxisLabel, LadderSetup, RunControls } from "@/components/GainsBlock";
import { GuidedTune } from "@/components/GuidedTune";
import { LadderPlot } from "@/components/LadderPlot";
import { LivePanels } from "@/components/LivePanels";
import { MAX_OVERLAY, PALETTE } from "@/components/PlotPanel";
import { PsdPlot } from "@/components/PsdPlot";
import { RestBars } from "@/components/RestBars";
import { finishedMs, ResultsTable } from "@/components/ResultsTable";
import { TracePanels, type OverlayRun } from "@/components/TracePanels";
import { Button, Card } from "@/components/ui";
import { useFeed } from "@/feed";
import { workbench, type WorkbenchState } from "@/workbench";
import {
  assignColors,
  bedLimits,
  browserStorage,
  buildLadderRequest,
  declaredMetrics,
  fallbackPattern,
  DEFAULT_PARAMETERS,
  defaultLadderValues,
  headlineMetric,
  ladderMedians,
  loadPattern,
  nextSession,
  parseNumberList,
  patternProblem,
  probeEdgeHint,
  probeToggleSpacing,
  isUnknownSession,
  pickRestoreSession,
  restoreStillApplies,
  type RestoreBasis,
  sessionForRun,
  setupOfSession,
  RECORDING_STATES,
  runTypeOfKind,
  savePattern,
  stateLabel,
  TERMINAL_STATES,
  toggleSpacingProblem,
  type LadderInput,
  type ProbeEdgeHint,
  type RunBody,
  type RunType,
  type Setup,
} from "@/tune";
import { Route as rootRoute } from "./__root";

export const Route = createRoute({
  getParentRoute: () => rootRoute,
  path: "/tune",
  component: TunePage,
});

const RUN_TYPES: { type: RunType; label: string }[] = [
  { type: "tracking", label: "Tracking" },
  { type: "probe", label: "Probe" },
  { type: "standstill", label: "Standstill" },
];

const DEFAULT_LADDER: LadderInput = {
  axis: "velocity_p",
  text: "",
  repeats: 2,
  floor: 0.05,
  marginal: 0.08,
};

const MODES: { mode: "manual" | "guided"; label: string }[] = [
  { mode: "manual", label: "Manual" },
  { mode: "guided", label: "Guided" },
];

const AWAIT_PICKUP_MS = 10_000;

const EDGE_HINT_CLASS: Record<ProbeEdgeHint["tone"], string> = {
  ok: "text-xs text-muted-foreground",
  caution: "rounded border border-amber-500/60 px-2 py-1 text-xs text-amber-300",
  warning: "rounded border border-red-500/60 px-2 py-1 text-xs text-red-300",
};

/** State kept in the module-level workbench store, so it survives leaving the page. */
function useStored<K extends keyof WorkbenchState>(
  key: K,
  initial: () => WorkbenchState[K],
): [WorkbenchState[K], (v: WorkbenchState[K] | ((prev: WorkbenchState[K]) => WorkbenchState[K])) => void] {
  const [value, setValue] = useState<WorkbenchState[K]>(() =>
    key in workbench ? (workbench[key] as WorkbenchState[K]) : initial(),
  );
  useEffect(() => {
    workbench[key] = value;
  }, [key, value]);
  return [value, setValue];
}

function errorText(e: unknown): string {
  if (e instanceof ApiRequestError) return e.field === null ? e.message : `${e.field}: ${e.message}`;
  return e instanceof Error ? e.message : "request failed";
}

function channelOf(printer: PrinterView, stepper: StepperView | undefined) {
  if (stepper === undefined) return undefined;
  return printer.boards.find((b) => b.serial === stepper.serial)?.channels.find((c) => c.channel === stepper.channel);
}

/** Refetches a session whenever the engine reports on it, dropping out-of-order replies. */
function useSessionView(id: SessionId | null, onGone: () => void) {
  const { listen } = useFeed();
  const [view, setView] = useState<SessionView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);

  const refresh = useCallback(() => {
    if (id === null) return;
    const mine = ++generation.current;
    getSession(id).then(
      (v) => {
        if (mine !== generation.current) return;
        setView(v);
        setError(null);
      },
      (e: unknown) => {
        if (mine !== generation.current) return;
        if (isUnknownSession(e)) onGone();
        else setError(errorText(e));
      },
    );
  }, [id, onGone]);

  useEffect(() => {
    setView(null);
    refresh();
  }, [refresh]);

  useEffect(
    () =>
      listen((m) => {
        if ((m.type === "run_state" || m.type === "run_result") && m.session === id) refresh();
        if (m.type === "resync") refresh();
      }),
    [listen, id, refresh],
  );

  return { view: id === null ? null : view, error, refresh };
}

function TunePage() {
  const { printer, loadError } = useFeed();
  const [mode, setMode] = useState<"manual" | "guided">("manual");
  if (printer === null) {
    return (
      <main className="mx-auto max-w-6xl px-4 py-6">
        <h1 className="text-lg font-medium text-muted-foreground">{loadError ?? "Waiting for the printer snapshot"}</h1>
      </main>
    );
  }
  if (printer.steppers.length === 0) {
    return (
      <main className="mx-auto max-w-6xl px-4 py-6">
        <h1 className="text-lg font-medium text-muted-foreground">
          {printer.settings_error ?? "No FOCI steppers found"}
        </h1>
      </main>
    );
  }
  return (
    <main className="mx-auto max-w-7xl space-y-4 px-4 py-6">
      <div role="radiogroup" aria-label="Tune mode" className="flex gap-1">
        {MODES.map((m) => (
          <button
            key={m.mode}
            type="button"
            role="radio"
            aria-checked={mode === m.mode}
            onClick={() => setMode(m.mode)}
            className={
              mode === m.mode
                ? "rounded-full border border-emerald-500 px-3 py-1 text-sm text-emerald-300"
                : "rounded-full border px-3 py-1 text-sm text-muted-foreground"
            }
          >
            {m.label}
          </button>
        ))}
      </div>
      {mode === "guided" ? <GuidedTune printer={printer} /> : <Workbench printer={printer} />}
    </main>
  );
}

function Workbench({ printer }: { printer: PrinterView }) {
  const storage = useMemo(() => browserStorage(), []);
  const limits = useMemo(() => bedLimits(printer.steppers), [printer.steppers]);
  const { refetch } = useFeed();
  const [stepperName, setStepperName] = useStored(
    "stepperName",
    () => (printer.steppers.find((s) => s.unusable === null) ?? printer.steppers[0]).name,
  );
  const stepper = printer.steppers.find((s) => s.name === stepperName);
  const channel = channelOf(printer, stepper);
  const pwmHz = channel !== undefined && channel.control.pwm_hz > 0 ? channel.control.pwm_hz : null;

  const [params, setParams] = useStored("params", () => DEFAULT_PARAMETERS);
  const [type, setType] = useStored<"type">("type", () => "probe");
  const [patterns, setPatterns] = useStored<"patterns">("patterns", () => ({}));
  const [speedsText, setSpeedsText] = useStored("speedsText", () =>
    DEFAULT_PARAMETERS.tracking.speeds_mm_s.join(", "),
  );

  const patternFor = (s: StepperView | undefined): Pattern | null => {
    if (s === undefined || limits === null) return null;
    const typed = patterns[s.name];
    if (typed !== undefined) return typed;
    const remembered = loadPattern(storage, s.name);
    if (remembered !== null && patternProblem(remembered, limits) === null) return remembered;
    return s.proposal.pattern ?? fallbackPattern(limits);
  };
  const pattern = patternFor(stepper);
  const patternError = pattern !== null && limits !== null ? patternProblem(pattern, limits) : null;

  const speeds = parseNumberList(speedsText, "the speed set");
  const runSetup: RunSetup =
    type === "tracking"
      ? { type, parameters: { ...params.tracking, speeds_mm_s: speeds.ok ? speeds.values : [] } }
      : type === "probe"
        ? { type, parameters: params.probe }
        : { type, parameters: params.standstill };
  const setup: Setup | null =
    stepper === undefined || pattern === null ? null : { stepper: stepper.name, pattern, ...runSetup };

  const updateSetup = (next: Setup) => setParams((prev) => ({ ...prev, [next.type]: next.parameters }));

  const setPattern = (p: Pattern) => {
    if (stepper === undefined) return;
    setPatterns((prev) => ({ ...prev, [stepper.name]: p }));
    if (limits !== null && patternProblem(p, limits) === null) savePattern(storage, stepper.name, p);
  };

  const adopt = (s: Setup) => {
    setStepperName(s.stepper);
    setType(s.type);
    setParams((prev) => ({ ...prev, [s.type]: s.parameters }));
    if (s.type === "tracking") setSpeedsText(s.parameters.speeds_mm_s.join(", "));
    setPatterns((prev) => ({ ...prev, [s.stepper]: s.pattern }));
  };

  const [gains, setGains] = useStored<"gains">("gains", () => null);
  const [filters, setFilters] = useStored<"filters">("filters", () => null);
  const [seededFor, setSeededFor] = useStored<"seededFor">("seededFor", () => null);
  const channelKey = stepper === undefined ? null : `${stepper.serial}:${stepper.channel}`;
  const seed = useCallback(() => {
    if (channel === undefined) return;
    const c = channel.control;
    setGains({ velocity_p: c.velocity_p, velocity_i: c.velocity_i, position_p: c.position_p });
    setFilters({
      velocity_hz: c.velocity_filter_hz,
      torque_hz: c.torque_filter_hz,
      position_hz: c.position_filter_hz,
      flux_hz: c.flux_filter_hz,
    });
  }, [channel, setGains, setFilters]);
  const [reseed, setReseed] = useState(false);
  useEffect(() => {
    if (channel === undefined) return;
    if (seededFor !== channelKey || reseed) {
      setSeededFor(channelKey);
      setReseed(false);
      seed();
    }
  }, [channel, channelKey, seededFor, reseed, seed, setSeededFor]);

  const [ladderOn, setLadderOn] = useStored<"ladderOn">("ladderOn", () => false);
  const [ladder, setLadder] = useStored<"ladder">("ladder", () => DEFAULT_LADDER);

  const current = printer.engine.current;
  const queue = printer.engine.queue;

  const [link, setLink] = useStored<"link">("link", () => null);
  const formKey = setup === null ? "" : JSON.stringify(setup);
  const basis = useRef<RestoreBasis>({ link: null, form: formKey });
  basis.current = { link: link?.id ?? null, form: formKey };
  const restoring = useRef(false);
  useEffect(() => {
    if (link !== null || restoring.current || setup === null) return;
    if (current === null && queue.length === 0) return;
    restoring.current = true;
    const started = basis.current;
    const running = current?.session ?? null;
    getSessions().then(
      (sessions) => {
        if (!restoreStillApplies(started, basis.current)) return;
        const id = pickRestoreSession(running, queue, sessions);
        const found = sessions.find((v) => v.id === id);
        const restored = found === undefined ? null : setupOfSession(found);
        if (id === null || restored === null) return;
        if (nextSession(restored, setup)) adopt(restored);
        setLink({ id, setup: restored });
      },
      () => undefined,
    );
  });

  const [message, setMessage] = useState<{ tone: "ok" | "error"; text: string } | null>(null);
  const forgetSession = useCallback(() => {
    setLink(null);
    setMessage({
      tone: "error",
      text: "The linked session no longer exists; the next Run opens a new session",
    });
  }, [setLink]);

  const active = link !== null && setup !== null && sessionForRun(link, setup) !== null ? link : null;
  const runningLinked = current !== null && link !== null && current.session === link.id;
  const shown = active ?? (runningLinked ? link : null);
  const { view, error: sessionError, refresh } = useSessionView(shown?.id ?? null, forgetSession);

  const [busy, setBusy] = useState(false);
  const [awaiting, setAwaiting] = useState<string[] | null>(null);

  useEffect(() => {
    if (awaiting === null) return;
    const timer = setTimeout(() => setAwaiting(null), AWAIT_PICKUP_MS);
    return () => clearTimeout(timer);
  }, [awaiting]);
  const pickedUp =
    awaiting !== null &&
    ((current !== null && awaiting.includes(current.run)) ||
      queue.some((r) => awaiting.includes(r)) ||
      (view?.runs ?? []).some((r) => awaiting.includes(r.id) && TERMINAL_STATES.has(r.state)));
  useEffect(() => {
    if (pickedUp) setAwaiting(null);
  }, [pickedUp]);

  const precondition = printer.preconditions.find((p) => p.stepper === stepperName);

  const blocked: string[] = [];
  if (awaiting !== null) blocked.push("waiting for the engine to take up the queued run");
  if (precondition !== undefined && !precondition.ok) blocked.push(...precondition.failing);
  if (stepper?.unusable) blocked.push(stepper.unusable);
  if (limits === null) blocked.push("bed limits need stepper_x and stepper_y in the config");
  if (patternError !== null) blocked.push(patternError);
  if (gains === null || filters === null) blocked.push("no control-state frame for the stepper's channel yet");
  if (gains !== null && filters !== null) blocked.push(...gainProblems(gains, filters));
  if (type === "tracking" && !speeds.ok) blocked.push(speeds.error);
  const spacing =
    type === "probe" && pattern !== null && stepper !== undefined
      ? probeToggleSpacing(
          pattern,
          stepper.kinematics,
          params.probe.speed_mm_s,
          params.probe.segments_between_toggles,
          printer.moonraker.max_velocity,
        )
      : null;
  const spacingProblem = spacing === null ? null : toggleSpacingProblem(spacing);
  if (spacingProblem !== null) blocked.push(spacingProblem);
  const edgeHint =
    type === "probe" && pattern !== null && stepper !== undefined
      ? probeEdgeHint({
          pattern,
          kinematics: stepper.kinematics,
          speedMmS: params.probe.speed_mm_s,
          segmentsBetweenToggles: params.probe.segments_between_toggles,
          strokes: params.probe.strokes,
          maxVelocity: printer.moonraker.max_velocity,
        })
      : null;

  const runBody = (): RunBody | null =>
    setup === null || gains === null || filters === null
      ? null
      : ({
          session: sessionForRun(link, setup),
          stepper: setup.stepper,
          type: setup.type,
          pattern: setup.pattern,
          parameters: setup.parameters,
          gains,
          filters,
        } as RunBody);

  const ladderBuilt = (() => {
    const base = runBody();
    return ladderOn && base !== null ? buildLadderRequest(base, ladder, pwmHz) : null;
  })();
  if (ladderBuilt !== null && !ladderBuilt.ok) blocked.push(ladderBuilt.error);
  const uniqueBlocked = [...new Set(blocked)];

  const submit = () => {
    const base = runBody();
    if (base === null || setup === null) return;
    setBusy(true);
    setMessage(null);
    const started = setup;
    const done = (id: SessionId, runs: string[]) => {
      setLink({ id, setup: started });
      setAwaiting(runs);
      setMessage({ tone: "ok", text: runs.length === 1 ? "Run queued" : `${runs.length} runs queued` });
      refresh();
    };
    const request =
      ladderBuilt !== null && ladderBuilt.ok
        ? postLadder(ladderBuilt.body).then((r) => done(r.session, r.runs))
        : postRun(base).then((r) => done(r.session, [r.run]));
    request
      .catch((e: unknown) => {
        if (isUnknownSession(e)) forgetSession();
        else setMessage({ tone: "error", text: errorText(e) });
      })
      .finally(() => setBusy(false));
  };

  const revert = () => {
    if (active === null) return;
    setMessage(null);
    postRevert(active.id).then(
      () => {
        setMessage({
          tone: "ok",
          text: "The session's starting gains and filters were written back; the gains block reloads them",
        });
        setReseed(true);
        refetch();
      },
      (e: unknown) => setMessage({ tone: "error", text: `Revert: ${errorText(e)}` }),
    );
  };

  const abort = () => {
    if (current === null) return;
    postAbort(current.run).then(
      (r) => setMessage({ tone: "ok", text: `Aborted ${r.aborted.length} queued runs; the current run finishes` }),
      (e: unknown) => setMessage({ tone: "error", text: `Abort: ${errorText(e)}` }),
    );
  };

  const busyEngine = current !== null || queue.length > 0;
  const recording =
    current !== null && shown !== null && current.session === shown.id && RECORDING_STATES.has(current.state);
  const recordingStepper = shown === null ? undefined : printer.steppers.find((s) => s.name === shown.setup.stepper);
  const pending = (view?.runs ?? []).filter((r) => !TERMINAL_STATES.has(r.state));
  const currentRecord = current === null ? null : (view?.runs.find((r) => r.id === current.run)?.run ?? null);

  const changedSetup = link !== null && active === null;

  return (
    <div className="space-y-4">
      <h1 className="sr-only">Tune</h1>
      <div className="grid gap-4 lg:grid-cols-2">
        <Card label="Setup">
          <div className="space-y-3">
            <div className="flex flex-wrap gap-3">
              <label className="flex flex-col text-xs text-muted-foreground">
                Stepper
                <select
                  value={stepperName}
                  disabled={busy}
                  onChange={(e) => setStepperName(e.target.value)}
                  className="rounded border bg-background px-1.5 py-1 text-sm text-foreground"
                >
                  {printer.steppers.map((s) => (
                    <option key={s.name} value={s.name}>
                      {s.name}
                      {s.unusable !== null ? " (unusable)" : ""}
                    </option>
                  ))}
                </select>
              </label>
              <div role="radiogroup" aria-label="Run type" className="flex items-end gap-1">
                {RUN_TYPES.map((t) => (
                  <button
                    key={t.type}
                    type="button"
                    role="radio"
                    aria-checked={type === t.type}
                    disabled={busy}
                    onClick={() => setType(t.type)}
                    className={
                      type === t.type
                        ? "rounded-full border border-emerald-500 px-3 py-1 text-sm text-emerald-300"
                        : "rounded-full border px-3 py-1 text-sm text-muted-foreground"
                    }
                  >
                    {t.label}
                  </button>
                ))}
              </div>
            </div>
            {setup !== null && (
              <ParametersBlock
                type={type}
                setup={setup}
                speedsText={speedsText}
                disabled={busy}
                onSpeedsText={setSpeedsText}
                onSetup={updateSetup}
              />
            )}
            {spacingProblem !== null && <p className="text-xs text-amber-400">{spacingProblem}</p>}
            {edgeHint !== null && (
              <p role="note" className={EDGE_HINT_CLASS[edgeHint.tone]}>
                {edgeHint.text}
              </p>
            )}
            {stepper?.proposal.refusal != null && (
              <p className="text-xs text-amber-400">
                No proposed pattern: {stepper.proposal.refusal}. The pattern is yours to choose.
              </p>
            )}
            {limits !== null && pattern !== null && stepper !== undefined && (
              <div>
                <BedMap
                  limits={limits}
                  pattern={pattern}
                  proposal={stepper.proposal.pattern}
                  disabled={busy}
                  onChange={setPattern}
                />
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
            <p className="text-xs text-muted-foreground">
              {active !== null
                ? `Session ${active.id}. Changing the setup starts a new session.`
                : changedSetup
                  ? "The setup differs from the last session; the next run starts a new session."
                  : "The first run starts a new session."}
            </p>
          </div>
        </Card>

        <Card label="Gains">
          {gains === null || filters === null ? (
            <p className="text-sm text-muted-foreground">Waiting for a control-state frame from the channel.</p>
          ) : (
            <div className="space-y-4">
              <GainsBlock
                gains={gains}
                filters={filters}
                positionI={
                  view?.session != null
                    ? { value: view.session.gains.position_i, source: "the session's baseline" }
                    : { value: channel?.control.position_i ?? 0, source: "live; a session holds its baseline" }
                }
                pwmHz={pwmHz}
                disabled={busy}
                ladder={ladder}
                ladderOn={ladderOn}
                ladderDefaults={pwmHz === null && ladder.axis === "velocity_i_corner_hz" ? [] : defaultLadderValues(ladder.axis, gains, pwmHz ?? 0)}
                onGains={setGains}
                onFilters={setFilters}
                onLadder={setLadder}
              />
              <Button className="text-xs" disabled={busy || channel === undefined} onClick={seed}>
                Load the live values
              </Button>
              <LadderSetup
                on={ladderOn}
                ladder={ladder}
                probe={type === "probe"}
                disabled={busy}
                onToggle={setLadderOn}
                onLadder={setLadder}
              />
            </div>
          )}
          <div className="mt-4 flex flex-wrap items-start gap-3 border-t pt-4">
            <RunControls label={ladderOn ? "Run ladder" : "Run"} blocked={uniqueBlocked} busy={busy} onRun={submit} />
            <Button disabled={active === null || busyEngine} onClick={revert}>
              Revert
            </Button>
          </div>
          {message !== null && (
            <p role="status" className={message.tone === "ok" ? "mt-2 text-sm text-emerald-400" : "mt-2 text-sm text-red-400"}>
              {message.text}
            </p>
          )}
        </Card>
      </div>

      {(current !== null || queue.length > 0) && (
        <Card label="Engine">
          <div className="flex flex-wrap items-center gap-4 text-sm">
            {current !== null && (
              <span>
                Run <span className="font-mono">{current.run}</span>:{" "}
                <span className="text-emerald-300">{stateLabel(current.state)}</span>
                {"reason" in current && <span className="text-muted-foreground"> ({current.reason})</span>}
                {active !== null && current.session !== active.id && (
                  <span className="text-muted-foreground"> in another session</span>
                )}
              </span>
            )}
            <span className="text-muted-foreground">{queue.length} queued</span>
            {current !== null && (
              <Button onClick={abort}>Abort after this run</Button>
            )}
          </div>
          {pending.length > 0 && (
            <ol aria-label="Queue" className="mt-2 flex flex-wrap gap-2 text-xs">
              {pending.map((r) => (
                <li key={r.id} className="rounded border px-2 py-0.5 font-mono">
                  {r.run?.request.ladder_value != null ? `${r.run.request.ladder_value}: ` : ""}
                  {stateLabel(r.state)}
                </li>
              ))}
            </ol>
          )}
        </Card>
      )}

      {recording && recordingStepper !== undefined && current !== null && (
        <Card label="Live">
          <LivePanels key={current.run} stepper={recordingStepper} runRegime={currentRecord?.regime ?? null} />
        </Card>
      )}

      {sessionError !== null && <p className="text-sm text-red-400">Session: {sessionError}</p>}
      {view !== null && view.session !== null && (
        <SessionResults view={view} />
      )}
    </div>
  );
}

function setupType(view: SessionView): RunType {
  const kind = view.session?.kind;
  return kind === undefined ? "tracking" : runTypeOfKind(kind);
}

/** Ladder plot, results table, overlay plots and Accept for one session. */
export function SessionResults({ view }: { view: SessionView }) {
  const type = setupType(view);
  const entries = useMemo(() => view.runs.filter((r) => TERMINAL_STATES.has(r.state)), [view]);
  const [checked, setChecked] = useState<string[]>([]);
  const [colors, setColors] = useState<Map<string, number>>(new Map());
  const [series, setSeries] = useState<Record<string, Series | { error: string }>>({});
  const [config, setConfig] = useState<{ run: string; text: string } | { error: string } | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    setChecked([]);
    setColors(new Map());
    setConfig(null);
  }, [view.id]);

  const toggle = (id: string, on: boolean) => {
    const next = on ? [...checked, id].slice(0, MAX_OVERLAY) : checked.filter((c) => c !== id);
    setChecked(next);
    setColors((prev) => assignColors(prev, next));
  };

  useEffect(() => {
    for (const id of checked) {
      if (series[id] !== undefined) continue;
      getSeries(id).then(
        (s) => setSeries((prev) => ({ ...prev, [id]: s })),
        (e: unknown) => setSeries((prev) => ({ ...prev, [id]: { error: errorText(e) } })),
      );
    }
  }, [checked, series]);

  const ladder = view.session?.ladder ?? null;
  const ladderEntries = useMemo(
    () =>
      [...entries]
        .filter(
          (e) =>
            e.run !== null && e.run.request.ladder_value !== null && e.state !== "skipped" && e.state !== "aborted",
        )
        .sort((a, b) => finishedMs(a) - finishedMs(b))
        .map((e) => ({
          value: e.run?.request.ladder_value ?? null,
          metric: e.run === null ? null : headlineMetric(e.run, type).value,
          assessable: e.state === "done",
        })),
    [entries, type],
  );
  const repeats = ladder?.repeats ?? 1;
  const unassessable = ladderMedians(ladderEntries, repeats)
    .filter((m) => m.unassessable)
    .map((m) => m.value);
  const headlineLabel = type === "probe" ? "Velocity zeta" : type === "tracking" ? "p99 at top speed (um)" : "Worst torque sd";

  const overlay = useMemo(() => checked.flatMap((id) => {
    const entry = entries.find((e) => e.id === id);
    const s = series[id];
    if (entry?.run == null || s === undefined || "error" in s) return [];
    const plot = s.channels.find((c) => c.channel === entry.run?.declared_channel);
    if (plot === undefined) return [];
    const label = new Date(finishedMs(entry)).toLocaleTimeString([], { hour12: false });
    return [{ id, label, color: PALETTE[colors.get(id) ?? 0], plot, run: entry.run }];
  }), [checked, entries, series, colors]);
  const seriesErrors = checked.flatMap((id) => {
    const s = series[id];
    return s !== undefined && "error" in s ? [`${id}: ${s.error}`] : [];
  });
  const plots = useMemo(() => {
    const trace: OverlayRun<ChannelPlot>[] = overlay.map((o) => ({ ...o, value: o.plot }));
    const ensembles = overlay.flatMap((o) => (o.plot.probe === null ? [] : [{ ...o, value: o.plot.probe }]));
    const psd = overlay.flatMap((o) => (o.plot.torque_psd === null ? [] : [{ ...o, value: o.plot.torque_psd }]));
    const rests = overlay.flatMap((o) => {
      const r = declaredMetrics(o.run)?.standstill?.rests;
      return r === undefined ? [] : [{ ...o, value: r }];
    });
    return { trace, ensembles, psd, rests };
  }, [overlay]);
  const { trace, ensembles, psd, rests } = plots;

  const accept = () => {
    if (checked.length !== 1) return;
    const run = checked[0];
    getConfigBlock(run).then(
      (text) => {
        setConfig({ run, text });
        setCopied(false);
      },
      (e: unknown) => setConfig({ error: errorText(e) }),
    );
  };

  const copy = (text: string) => {
    try {
      navigator.clipboard.writeText(text).then(
        () => setCopied(true),
        () => setCopied(false),
      );
    } catch {
      setCopied(false);
    }
  };

  const acceptedEntry = config !== null && "run" in config ? entries.find((e) => e.id === config.run) : undefined;

  return (
    <>
      {ladderEntries.length > 0 && (
        <Card label="Ladder">
          <LadderPlot
            entries={ladderEntries}
            repeats={repeats}
            axisLabel={ladderAxisLabel(ladder?.axis ?? "")}
            metricLabel={headlineLabel}
            floor={type === "probe" && ladder !== null ? ladder.floor : null}
            marginal={type === "probe" && ladder !== null ? ladder.marginal : null}
          />
          {type === "probe" && unassessable.length > 0 && (
            <p className="mt-2 text-sm text-amber-400">
              No median for {unassessable.join(", ")}: a repeat did not finish done with a metric, so the value is
              unassessable.
            </p>
          )}
        </Card>
      )}
      <Card label={`Results, ${view.session?.name ?? view.id}`}>
        <ResultsTable
          entries={entries}
          type={type}
          ladderAxis={ladder?.axis ?? null}
          checked={new Set(checked)}
          colors={colors}
          onToggle={toggle}
        />
        <div className="mt-3 flex flex-wrap items-center gap-3">
          <Button disabled={checked.length !== 1} onClick={accept}>
            Accept
          </Button>
          <span className="text-xs text-muted-foreground">
            {checked.length === 1
              ? "Shows the config block for the checked run."
              : "Check exactly one run to accept it."}
            {checked.length >= MAX_OVERLAY && ` At most ${MAX_OVERLAY} runs overlay at once.`}
          </span>
        </div>
        {config !== null && "error" in config && <p className="mt-2 text-sm text-red-400">Accept: {config.error}</p>}
        {config !== null && "text" in config && (
          <div className="mt-3 space-y-2">
            {acceptedEntry !== undefined && acceptedEntry.state !== "done" && (
              <p className="text-sm text-amber-400">This run is {stateLabel(acceptedEntry.state)}; check its gains before using them.</p>
            )}
            <pre aria-label="Config block" className="overflow-x-auto rounded-md border bg-background p-3 font-mono text-xs select-all">
              {config.text}
            </pre>
            <Button onClick={() => copy(config.text)}>{copied ? "Copied" : "Copy"}</Button>
          </div>
        )}
      </Card>
      {checked.length > 0 && (
        <Card label="Plots">
          {seriesErrors.map((e) => (
            <p key={e} className="text-sm text-red-400">
              {e}
            </p>
          ))}
          {trace.length > 0 && <TracePanels runs={trace} />}
          {ensembles.length > 0 && (
            <div className="mt-4">
              <h3 className="mb-1 text-xs uppercase tracking-wider text-muted-foreground">Step-response ensembles</h3>
              <EnsemblePlot runs={ensembles} />
            </div>
          )}
          {psd.length > 0 && (
            <div className="mt-4">
              <h3 className="mb-1 text-xs uppercase tracking-wider text-muted-foreground">Torque PSD</h3>
              <PsdPlot runs={psd} />
            </div>
          )}
          {rests.length > 0 && (
            <div className="mt-4">
              <h3 className="mb-1 text-xs uppercase tracking-wider text-muted-foreground">Rests</h3>
              <RestBars runs={rests} />
            </div>
          )}
        </Card>
      )}
    </>
  );
}
