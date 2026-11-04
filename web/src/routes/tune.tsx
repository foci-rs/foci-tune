import { createRoute, Outlet, redirect, useNavigate } from "@tanstack/react-router";
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
  type LadderAxis,
  type Pattern,
  type PrinterView,
  type RunSetup,
  type Series,
  type SessionId,
  type SessionView,
  type StepperView,
} from "@/api";
import { ParametersBlock } from "@/components/ParametersBlock";
import { EnsemblePlot } from "@/components/EnsemblePlot";
import { FiltersCard, gainProblems, GainsCard, ladderAxisLabel } from "@/components/GainsBlock";
import { ProbeNotes } from "@/components/EdgeHint";
import { RunDetails } from "@/components/RunDetails";
import { RunQueueCard } from "@/components/RunQueueCard";
import { SessionCard } from "@/components/SessionCard";
import { Steps } from "@/components/Steps";
import { sweepLadderEntries, sweepNotesFor } from "@/sweep";
import { TestCard } from "@/components/TestCard";
import { LadderCard, PatternCard, RunBar } from "@/components/WorkbenchCards";
import { GuidedTune } from "@/components/GuidedTune";
import { LadderPlot } from "@/components/LadderPlot";
import { LivePanels } from "@/components/LivePanels";
import { MAX_OVERLAY, PALETTE } from "@/components/PlotPanel";
import { PsdPlot } from "@/components/PsdPlot";
import { RestBars } from "@/components/RestBars";
import { finishedMs, ResultsTable } from "@/components/ResultsTable";
import { TracePanels, type OverlayRun } from "@/components/TracePanels";
import { Card } from "@/components/ui";
import { useFeed } from "@/feed";
import { useNow } from "@/useNow";
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
  ladderFieldErrors,
  loadPattern,
  patternIsolation,
  patternMode,
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
  queueInOrder,
  runPosition,
  setupOfSession,
  sweepOrder,
  RECORDING_STATES,
  runTypeOfKind,
  savePattern,
  TERMINAL_STATES,
  toggleSpacingProblem,
  type LadderInput,
  type PatternMode,
  type RunBody,
  type RunType,
  type Setup,
} from "@/tune";
import { Route as rootRoute } from "./__root";

export const Route = createRoute({
  getParentRoute: () => rootRoute,
  path: "/tune",
  beforeLoad: ({ location }) => {
    if (location.pathname === "/tune" || location.pathname === "/tune/") {
      throw redirect({ to: "/tune/manual" });
    }
  },
  component: TunePage,
});

export type ManualStep = "prepare" | "run";

export const ManualRoute = createRoute({
  getParentRoute: () => Route,
  path: "manual",
  validateSearch: (search: Record<string, unknown>): { step?: ManualStep } =>
    search.step === "prepare" || search.step === "run" ? { step: search.step } : {},
  component: ManualTune,
});

export const GuidedRoute = createRoute({
  getParentRoute: () => Route,
  path: "guided",
  validateSearch: (search: Record<string, unknown>): { tune?: string } =>
    typeof search.tune === "string" && search.tune !== "" ? { tune: search.tune } : {},
  component: GuidedTunePage,
});

const DEFAULT_LADDER: LadderInput = {
  axis: "velocity_p",
  text: "",
  repeats: 2,
  floor: 0.05,
  marginal: 0.08,
};

const AWAIT_PICKUP_MS = 10_000;

/** The engine's own wording for a refused run while another is in flight. */
const ENGINE_BUSY = "a run is in flight";

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
      <Outlet />
    </main>
  );
}

function ManualTune() {
  const { printer } = useFeed();
  return printer === null ? null : <Workbench printer={printer} />;
}

function GuidedTunePage() {
  const { printer } = useFeed();
  const { tune } = GuidedRoute.useSearch();
  return printer === null ? null : <GuidedTune key={tune ?? ""} printer={printer} initialTune={tune} />;
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
  const [speedsText, setSpeedsText] = useStored("speedsText", () => DEFAULT_PARAMETERS.tracking.speeds_mm_s.join(", "));

  const [patternModes, setPatternModes] = useStored<"patternModes">("patternModes", () => ({}));
  const rememberedFor = (s: StepperView): Pattern | null => {
    const typed = patterns[s.name];
    if (typed !== undefined) return typed;
    const remembered = loadPattern(storage, s.name);
    return remembered !== null && limits !== null && patternProblem(remembered, limits) === null ? remembered : null;
  };
  const modeFor = (s: StepperView): PatternMode =>
    patternMode(patternModes[s.name], s.proposal.pattern, rememberedFor(s));
  const patternFor = (s: StepperView | undefined): Pattern | null => {
    if (s === undefined || limits === null) return null;
    if (modeFor(s) === "default" && s.proposal.pattern !== null) return s.proposal.pattern;
    return rememberedFor(s) ?? s.proposal.pattern ?? fallbackPattern(limits);
  };
  const pattern = patternFor(stepper);
  const mode: PatternMode = stepper === undefined ? "custom" : modeFor(stepper);
  const chooseMode = (next: PatternMode) => {
    if (stepper === undefined) return;
    setPatternModes((prev) => ({ ...prev, [stepper.name]: next }));
    if (next === "default" && stepper.proposal.pattern !== null) setPattern(stepper.proposal.pattern);
    else if (pattern !== null) setPattern(pattern);
  };
  const otherSteppers =
    stepper === undefined
      ? []
      : printer.steppers.filter((s) => s.serial === stepper.serial && s.name !== stepper.name).map((s) => s.name);
  const isolation =
    stepper === undefined || pattern === null
      ? { ok: true, message: "", snapped: { start: [0, 0], end: [0, 0] } as Pattern, snapLabel: "" }
      : patternIsolation({
          pattern,
          stepper,
          others: otherSteppers,
          limits,
        });
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
    const proposal = printer.steppers.find((st) => st.name === s.stepper)?.proposal.pattern ?? null;
    setPatternModes((prev) => ({ ...prev, [s.stepper]: patternMode(undefined, proposal, s.pattern) }));
  };

  const [gains, setGains] = useStored<"gains">("gains", () => null);
  const [filters, setFilters] = useStored<"filters">("filters", () => null);
  const [seededFor, setSeededFor] = useStored<"seededFor">("seededFor", () => null);
  const channelKey = stepper === undefined ? null : `${stepper.serial}:${stepper.channel}`;
  const loadLiveGains = useCallback(() => {
    if (channel === undefined) return;
    const c = channel.control;
    setGains({ velocity_p: c.velocity_p, velocity_i: c.velocity_i, position_p: c.position_p });
  }, [channel, setGains]);
  const loadLiveFilters = useCallback(() => {
    if (channel === undefined) return;
    const c = channel.control;
    setFilters({
      velocity_hz: c.velocity_filter_hz,
      torque_hz: c.torque_filter_hz,
      position_hz: c.position_filter_hz,
      flux_hz: c.flux_filter_hz,
    });
  }, [channel, setFilters]);
  const seed = useCallback(() => {
    loadLiveGains();
    loadLiveFilters();
  }, [loadLiveGains, loadLiveFilters]);
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
  if (precondition !== undefined && !precondition.ok) {
    blocked.push(...precondition.failing.filter((reason) => reason !== ENGINE_BUSY));
  }
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
  const defaultsForAxis = (axis: LadderAxis): number[] =>
    gains === null || (pwmHz === null && axis === "velocity_i_corner_hz")
      ? []
      : defaultLadderValues(axis, gains, pwmHz ?? 0);
  const toggleSweep = (on: boolean) => {
    setLadderOn(on);
    if (on && ladder.text.trim() === "") setLadder({ ...ladder, text: defaultsForAxis(ladder.axis).join(", ") });
  };
  const sweepErrors = ladderFieldErrors(ladder, pwmHz, type === "probe");
  const sweepInvalid = ladderOn && (sweepErrors.values ?? sweepErrors.runs ?? sweepErrors.thresholds) !== null;
  if (sweepInvalid) blocked.push("Gain sweep: fix the highlighted fields");
  else if (ladderBuilt !== null && !ladderBuilt.ok) blocked.push(ladderBuilt.error);
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
      goTo("run");
      refresh();
    };
    const request = ladderBuilt?.ok
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
  const search = ManualRoute.useSearch();
  const navigate = useNavigate();
  const step: ManualStep = search.step ?? (busyEngine ? "run" : "prepare");
  const goTo = (next: string) => navigate({ to: "/tune/manual", search: { step: next as ManualStep } });
  const recording =
    current !== null && shown !== null && current.session === shown.id && RECORDING_STATES.has(current.state);
  const recordingStepper = shown === null ? undefined : printer.steppers.find((s) => s.name === shown.setup.stepper);
  const pending = queueInOrder(queue, view?.runs ?? []);
  const nowMs = useNow(current !== null);
  const position =
    current === null || view === null || current.session !== view.id ? null : runPosition(view.runs, current.run);
  const currentRecord = current === null ? null : (view?.runs.find((r) => r.id === current.run)?.run ?? null);

  const hasRun = busyEngine || (view !== null && view.session !== null);
  return (
    <div className="space-y-4">
      <h1 className="sr-only">Tune</h1>
      <Steps
        steps={[
          { id: "prepare", label: "Prepare" },
          { id: "run", label: "Run" },
        ]}
        current={step}
        onSelect={goTo}
      />
      {step === "prepare" ? (
        <>
          <div className="grid gap-4 lg:grid-cols-2">
            <TestCard
              steppers={printer.steppers}
              stepperName={stepperName}
              type={type}
              disabled={busy}
              onStepper={setStepperName}
              onType={setType}
            >
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
              <ProbeNotes spacingProblem={spacingProblem} hint={edgeHint} />
            </TestCard>
            <PatternCard
              limits={limits}
              pattern={pattern}
              proposal={stepper?.proposal.pattern ?? null}
              mode={mode}
              isolation={isolation.ok ? null : isolation}
              refusal={stepper?.proposal.refusal ?? null}
              error={patternError}
              notes={[]}
              disabled={busy}
              onPattern={setPattern}
              onMode={chooseMode}
            />
            <GainsCard
              gains={gains}
              positionI={view?.session != null ? view.session.gains.position_i : (channel?.control.position_i ?? 0)}
              live={channel?.control}
              pwmHz={pwmHz}
              disabled={busy}
              sweptAxis={ladderOn ? ladder.axis : null}
              canLoadLive={channel !== undefined}
              onGains={setGains}
              onLoadLive={loadLiveGains}
            />
            <FiltersCard
              filters={filters}
              disabled={busy}
              canLoadLive={channel !== undefined}
              onFilters={setFilters}
              onLoadLive={loadLiveFilters}
            />
          </div>
          <LadderCard
            on={ladderOn}
            ladder={ladder}
            errors={sweepErrors}
            defaultsFor={defaultsForAxis}
            probe={type === "probe"}
            disabled={busy}
            onToggle={toggleSweep}
            onLadder={setLadder}
          />
        </>
      ) : (
        <>
          {!hasRun && (
            <p className="text-sm text-muted-foreground">Nothing has run yet. Set up a run first, then press Run.</p>
          )}
          {(current !== null || queue.length > 0) && (
            <RunQueueCard
              current={current}
              kind={currentRecord === null ? undefined : runTypeOfKind(currentRecord.request.kind)}
              otherSession={current !== null && active !== null && current.session !== active.id}
              position={position}
              steps={currentRecord?.steps ?? []}
              predictedS={currentRecord?.predicted_s ?? null}
              nowMs={nowMs}
              pending={pending.map((r) => ({ id: r.id, state: r.state, value: r.run?.request.ladder_value ?? null }))}
              queued={queue.length}
            />
          )}

          {recording && recordingStepper !== undefined && current !== null && (
            <Card label="Live">
              <LivePanels key={current.run} stepper={recordingStepper} runRegime={currentRecord?.regime ?? null} />
            </Card>
          )}

          {sessionError !== null && <p className="text-sm text-red-400">Session: {sessionError}</p>}
          {view !== null && view.session !== null && (
            <>
              <SessionCard
                view={view}
                canRestore={active !== null && !busyEngine}
                onRestore={revert}
                notes={sweepNotesFor(view, printer)}
              />
              <SessionResults view={view} />
            </>
          )}
        </>
      )}
      <RunBar
        runLabel={busyEngine || awaiting !== null ? "Running" : ladderOn ? "Run sweep" : "Run"}
        blocked={step === "run" ? [] : uniqueBlocked}
        holdRun={!isolation.ok || awaiting !== null || busyEngine}
        mode={step}
        onBack={() => goTo("prepare")}
        busy={busy}
        onRun={submit}
        running={current !== null}
        onAbort={abort}
        message={message}
      />
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
  const entries = useMemo(
    () =>
      sweepOrder(
        view.runs.filter((r) => TERMINAL_STATES.has(r.state)).sort((a, b) => finishedMs(a) - finishedMs(b)),
        view.session?.ladder?.values ?? null,
      ),
    [view],
  );
  const [checked, setChecked] = useState<string[]>([]);
  const [colors, setColors] = useState<Map<string, number>>(new Map());
  const [series, setSeries] = useState<Record<string, Series | { error: string }>>({});
  const [config, setConfig] = useState<{ run: string; text: string } | { error: string } | null>(null);
  const [copied, setCopied] = useState(false);
  const [openRun, setOpenRun] = useState<string | null>(null);
  const { printer } = useFeed();

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
  const ladderEntries = useMemo(() => sweepLadderEntries(entries, type), [entries, type]);
  const repeats = ladder?.repeats ?? 1;
  const headlineLabel =
    type === "probe" ? "Velocity zeta" : type === "tracking" ? "p99 at top speed (μm)" : "Worst torque sd";

  const overlay = useMemo(
    () =>
      checked.flatMap((id) => {
        const entry = entries.find((e) => e.id === id);
        const s = series[id];
        if (entry?.run == null || s === undefined || "error" in s) return [];
        const plot = s.channels.find((c) => c.channel === entry.run?.declared_channel);
        if (plot === undefined) return [];
        const label = new Date(finishedMs(entry)).toLocaleTimeString([], { hour12: false });
        return [{ id, label, color: PALETTE[colors.get(id) ?? 0], plot, run: entry.run }];
      }),
    [checked, entries, series, colors],
  );
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

  const accept = (run: string) => {
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

  const tested = view.session?.stepper ?? "";
  const board = printer?.steppers.find((s) => s.name === tested)?.serial;
  const others = (printer?.steppers ?? []).filter((s) => s.serial === board && s.name !== tested).map((s) => s.name);

  return (
    <>
      {ladderEntries.length > 0 && (
        <Card label="Sweep results">
          <LadderPlot
            entries={ladderEntries}
            repeats={repeats}
            axisLabel={ladderAxisLabel(ladder?.axis ?? "")}
            metricLabel={headlineLabel}
            floor={type === "probe" && ladder !== null ? ladder.floor : null}
            marginal={type === "probe" && ladder !== null ? ladder.marginal : null}
          />
        </Card>
      )}
      <RunDetails
        entry={entries.find((e) => e.id === openRun) ?? null}
        type={type}
        ladderAxis={ladder?.axis ?? null}
        tested={tested}
        others={others}
        config={config}
        copied={copied}
        onAccept={accept}
        onCopy={copy}
        onClose={() => setOpenRun(null)}
      />
      <Card label={`Results, ${view.session?.name ?? view.id}`}>
        <ResultsTable
          entries={entries}
          type={type}
          ladderAxis={ladder?.axis ?? null}
          checked={new Set(checked)}
          colors={colors}
          tested={tested}
          others={others}
          limits={{ floor: ladder?.floor ?? 0.05, marginal: ladder?.marginal ?? 0.08 }}
          onToggle={toggle}
          onOpen={setOpenRun}
        />
        {checked.length >= MAX_OVERLAY && (
          <p className="mt-2 text-xs text-muted-foreground">At most {MAX_OVERLAY} runs overlay at once.</p>
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
