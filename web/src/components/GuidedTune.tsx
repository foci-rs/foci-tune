import { useCallback, useEffect, useRef, useState } from "react";
import {
  getTune,
  getTunes,
  postTuneAbandon,
  postTuneChoice,
  postTuneConfirm,
  postTuneDecision,
  postTuneExtension,
  postTuneRestore,
  postTuneStep,
  postTuneStepCancel,
  type EngineView,
  type PrinterView,
  type Tune,
  type TuneDetail,
  type TuneId,
  type TuneSummary,
} from "@/api";
import { ArrowRightIcon, PlayIcon } from "lucide-react";
import { Button as UiButton } from "@/components/ui/button";
import { failureMessage } from "@/failure";
import { useFeed } from "@/feed";
import {
  announcementOf,
  availableActions,
  axisLabel,
  axisOfStep,
  axisTitle,
  currentGuidedStep,
  currentStep,
  driftCheckRunning,
  failureText,
  formFromStep,
  GUIDED_STEPS,
  nextAxis,
  recommendedGain,
  rungView,
  stepSentence,
  standstillCheckNote,
  sweepProgress,
  stepSummary,
  DEFAULT_PREPARE,
  liveControlFor,
  liveSeed,
  plannedRuns,
  acceptGain,
  extensionOffer,
  measuredGains,
  nextGuidedStep,
  prepareState,
  type GuidedAction,
  type GuidedStepId,
  type PrepareForm,
  type PrepareState,
} from "@/guided";
import { GuidedCurve } from "./GuidedCurve";
import { GuidedSetup } from "./GuidedSetup";
import { ChooseGain } from "./ChooseGain";
import { NextStepCard } from "./NextStepCard";
import { PausePanel } from "./PausePanel";
import { Steps } from "./Steps";
import { SweepResultsTable } from "./SweepResultsTable";
import { TuneRunCard } from "./TuneRunCard";
import { ActionBar } from "./WorkbenchCards";
import { Button, Card } from "./ui";

const isOver = (s: TuneSummary) => s.state === "Done" || s.state === "Abandoned";

/** The guided tune: one tune at a time, step by step, with the operator confirming every motion. */
export function GuidedTune({ printer, initialTune }: { printer: PrinterView; initialTune?: TuneId }) {
  const { listen, connected } = useFeed();
  const offline = connected === false;
  const [summaries, setSummaries] = useState<TuneSummary[] | null>(null);
  const [selected, setSelected] = useState<TuneId | "new" | null>(initialTune ?? null);
  const [listError, setListError] = useState<string | null>(null);

  const load = useCallback(() => {
    getTunes().then(
      (list) => {
        setSummaries([...list].sort((a, b) => b.created - a.created));
        setListError(null);
      },
      (e: unknown) => setListError(failureMessage(e)),
    );
  }, []);
  useEffect(load, [load]);
  useEffect(
    () =>
      listen((m) => {
        if (m.type === "tune") load();
      }),
    [listen, load],
  );

  if (summaries === null) {
    return listError === null ? (
      <p className="text-sm text-muted-foreground">Loading tunes</p>
    ) : (
      <LoadFailure message={listError} onRetry={load} />
    );
  }
  const open = summaries.filter((s) => !isOver(s));
  const current = selected ?? open[0]?.id ?? "new";

  return (
    <div className="space-y-4">
      {listError !== null && <p className="text-sm text-red-400">{listError}</p>}
      {current === "new" ? (
        <NewTune
          printer={printer}
          offline={offline}
          onCreated={(tune) => {
            load();
            setSelected(tune.id);
          }}
        />
      ) : (
        <TuneView key={current} id={current} printer={printer} offline={offline} />
      )}
    </div>
  );
}

/** The setup form under the steps header; later steps wait for the tune, and the form keeps its values. */
function NewTune({
  printer,
  offline,
  onCreated,
}: {
  printer: PrinterView;
  offline: boolean;
  onCreated: (tune: Tune) => void;
}) {
  const [step, setStep] = useState<GuidedStepId>("setup");
  return (
    <div className="space-y-4">
      <Steps steps={GUIDED_STEPS} current={step} onSelect={(next) => setStep(next as GuidedStepId)} />
      <div hidden={step !== "setup"}>
        <GuidedSetup printer={printer} offline={offline} onCreated={onCreated} />
      </div>
      {step !== "setup" && <p className="text-sm text-muted-foreground">Create the tune first.</p>}
    </div>
  );
}

function useTune(id: TuneId) {
  const { listen } = useFeed();
  const [detail, setDetail] = useState<TuneDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const sequence = useRef(0);
  const refresh = useCallback(() => {
    const mine = ++sequence.current;
    getTune(id).then(
      (d) => {
        if (mine === sequence.current) {
          setDetail(d);
          setError(null);
        }
      },
      (e: unknown) => mine === sequence.current && setError(failureMessage(e)),
    );
  }, [id]);
  useEffect(refresh, [refresh]);
  useEffect(
    () =>
      listen((m) => {
        if ((m.type === "tune" && m.tune.id === id) || m.type === "resync") refresh();
      }),
    [listen, id, refresh],
  );
  return { detail, error, refresh };
}

function TuneView({ id, printer, offline }: { id: TuneId; printer: PrinterView; offline: boolean }) {
  const { detail, error, refresh } = useTune(id);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const announced = detail?.state === "Announced";
  const [prepare, setPrepare] = useState<PrepareForm>(DEFAULT_PREPARE);
  const [picked, setPicked] = useState<GuidedStepId | null>(null);
  const followed = detail === null ? null : currentGuidedStep(detail);
  useEffect(() => setPicked(null), [followed]);

  const run = (kind: GuidedAction, action: () => Promise<unknown>, after?: () => void) => {
    setBusy(true);
    setActionError(null);
    action()
      .then(() => after?.())
      .catch((e: unknown) =>
        getTune(id).then(
          (fresh) => setActionError(failureText(kind, e, detail, fresh)),
          () => setActionError(failureText(kind, e, detail, null)),
        ),
      )
      .finally(() => {
        setBusy(false);
        refresh();
      });
  };

  if (detail === null) {
    return error === null ? (
      <p className="text-sm text-muted-foreground">Loading the tune</p>
    ) : (
      <LoadFailure message={error} onRetry={refresh} />
    );
  }
  const frozen = busy || offline;
  const actions = availableActions(detail, detail.lock);
  const last = currentStep(detail);
  const shown = picked ?? followed ?? "setup";
  const axis = axisOfStep(shown);
  const lastAxis = last === null ? null : detail.steps[last].axis;
  const next = nextAxis(detail);
  const preparing = actions.prepare && next !== null && axis === next;
  const onward = followed === null ? null : nextGuidedStep(shown, followed);
  const choosing = actions.choose && axis !== null && lastAxis === axis && last !== null;
  const offer = choosing && actions.extension && last !== null ? extensionOffer(detail, last) : null;
  const recommended = choosing && last !== null ? recommendedGain(detail, last) : null;
  const deciding = actions.decide && axis !== null && lastAxis === axis;
  const starting = announced && actions.start && axis !== null && lastAxis === axis;
  const liveControl = liveControlFor(printer, detail.setup.stepper);
  const prepared = preparing && next !== null ? prepareState(next, prepare, liveControl) : null;
  const prepareBlocked = prepared?.problem ?? null;
  const plannedCount =
    prepared?.preview == null ? null : plannedRuns(prepared.preview, prepare.repeats, detail.setup.speeds_mm_s.length);
  const runs = plannedCount ?? undefined;
  const setupEditable = actions.prepare && detail.steps.length === 0;
  const announcedStep = starting && last !== null ? detail.steps[last] : null;
  const abandon = actions.abandon ? (
    <Button disabled={frozen} onClick={() => run("abandon", () => postTuneAbandon(id))}>
      Abandon tune
    </Button>
  ) : null;

  return (
    <div className="space-y-4">
      <Steps steps={GUIDED_STEPS} current={shown} onSelect={(next) => setPicked(next as GuidedStepId)} />

      {actions.restore && (
        <LockBanner
          detail={detail}
          busy={frozen}
          canRestore={actions.restore}
          onRestore={() => run("restore", () => postTuneRestore(id))}
        />
      )}

      {shown === "setup" && (
        <GuidedSetup
          printer={printer}
          tune={detail}
          readOnly={!setupEditable}
          offline={offline}
          onSaved={() => {
            refresh();
            setPicked(null);
          }}
          actions={abandon}
        />
      )}

      {axis !== null && (
        <AxisStep
          tune={detail}
          axis={axis}
          last={last}
          lastAxis={lastAxis}
          prepareAxis={actions.prepare ? next : null}
          actions={actions}
          busy={frozen}
          engine={printer.engine}
          announced={announced}
          prepare={prepare}
          prepared={prepared}
          runs={runs}
          live={next !== null && liveControl !== null ? liveSeed(next, liveControl) : null}
          onPrepare={setPrepare}
        />
      )}

      {shown === "result" &&
        (detail.config_block !== null ? (
          <Card label="Config block">
            <pre className="overflow-x-auto font-mono text-xs">{detail.config_block}</pre>
          </Card>
        ) : (
          <p className="text-sm text-muted-foreground">There is no result yet. It appears when the tune is done.</p>
        ))}

      {!(shown === "setup" && setupEditable) && (
        <ActionBar
          blocked={offline ? ["Reconnecting to foci-tune."] : prepareBlocked === null ? [] : [prepareBlocked]}
          message={actionError === null ? null : { tone: "error", text: actionError }}
        >
          {abandon}
          {choosing && last !== null && (
            <>
              {offer !== null && (
                <UiButton
                  variant="outline"
                  disabled={frozen}
                  onClick={() => run("extend", () => postTuneExtension(id, offer.direction))}
                >
                  {offer.direction === "below" ? "Try a lower gain" : "Try a higher gain"}
                </UiButton>
              )}
              <ChooseGain
                axis={detail.steps[last].axis}
                accept={acceptGain(detail, last)}
                initial={
                  recommended === null
                    ? null
                    : detail.steps[last].axis === "VelocityICornerHz"
                      ? Number(recommended.gain.toFixed(1))
                      : Math.round(recommended.gain)
                }
                gains={measuredGains(detail, last)}
                pwmHz={detail.steps[last].pwm_hz}
                busy={frozen}
                onAccept={() => run("choose", () => postTuneChoice(id, "recommended"))}
                onPick={(rung) => run("choose", () => postTuneChoice(id, { rung }))}
                onManual={(value) => run("choose", () => postTuneChoice(id, { value }))}
              />
            </>
          )}
          {deciding && (
            <>
              <UiButton
                variant="outline"
                disabled={frozen}
                onClick={() => run("decide", () => postTuneDecision(id, "stop"))}
              >
                Stop
              </UiButton>
              <UiButton
                variant="outline"
                disabled={frozen}
                onClick={() => run("decide", () => postTuneDecision(id, "rerun"))}
              >
                Rerun
              </UiButton>
              <UiButton disabled={frozen} onClick={() => run("decide", () => postTuneDecision(id, "continue"))}>
                Continue
              </UiButton>
            </>
          )}
          {onward !== null && (
            <UiButton onClick={() => setPicked(onward === followed ? null : onward)}>
              Next
              <ArrowRightIcon />
            </UiButton>
          )}
          {preparing && (
            <UiButton
              disabled={frozen || prepareBlocked !== null}
              onClick={() =>
                run("start", async () => {
                  await postTuneStep(id, {
                    seed: prepare.seed === null ? "live" : { typed: prepare.seed },
                    repeats: prepare.repeats,
                    lower_k: prepare.lower,
                    upper_k: prepare.upper,
                    octave_divisions: prepare.divisions,
                  });
                  try {
                    await postTuneConfirm(id);
                  } catch (e) {
                    await postTuneStepCancel(id).catch(() => undefined);
                    throw e;
                  }
                })
              }
              className="border border-emerald-600 bg-emerald-700 text-white hover:bg-emerald-600"
            >
              <PlayIcon />
              Run
            </UiButton>
          )}
          {starting && announcedStep !== null && (
            <>
              <UiButton
                variant="outline"
                disabled={frozen}
                onClick={() => {
                  setPrepare(
                    formFromStep(
                      announcedStep,
                      liveControl === null ? null : liveSeed(announcedStep.axis, liveControl),
                    ),
                  );
                  run("adjust", () => postTuneStepCancel(id));
                }}
              >
                Adjust
              </UiButton>
              <UiButton
                disabled={frozen}
                onClick={() => run("start", () => postTuneConfirm(id))}
                className="border border-emerald-600 bg-emerald-700 text-white hover:bg-emerald-600"
              >
                <PlayIcon />
                Run
              </UiButton>
            </>
          )}
        </ActionBar>
      )}
    </div>
  );
}

function AxisStep({
  tune,
  axis,
  last,
  lastAxis,
  prepareAxis,
  actions,
  busy,
  engine,
  announced,
  prepare,
  prepared,
  runs,
  live,
  onPrepare,
}: {
  tune: TuneDetail;
  axis: NonNullable<ReturnType<typeof nextAxis>>;
  last: number | null;
  lastAxis: string | null;
  prepareAxis: ReturnType<typeof nextAxis>;
  actions: ReturnType<typeof availableActions>;
  busy: boolean;
  engine: EngineView;
  announced: boolean;
  prepare: PrepareForm;
  prepared: PrepareState | null;
  runs: number | undefined;
  live: number | null;
  onPrepare: (form: PrepareForm) => void;
}) {
  const indices = tune.steps.flatMap((s, i) => (s.axis === axis ? [i] : []));
  const preparing = prepareAxis === axis;
  const announcing = announced && actions.start && lastAxis === axis;
  if (indices.length === 0 && !preparing) {
    return <p className="text-sm text-muted-foreground">{axisTitle(axis)} is not reached yet.</p>;
  }
  const sessions = lastAxis === axis && last !== null ? Object.values(tune.steps[last].sessions) : [];
  return (
    <>
      <TuneRunCard
        engine={engine}
        sessions={sessions}
        kinds={
          lastAxis === axis && last !== null
            ? {
                [tune.steps[last].sessions.probe]: "probe",
                [tune.steps[last].sessions.tracking]: "tracking",
                [tune.steps[last].sessions.standstill]: "standstill",
              }
            : undefined
        }
        sweep={tune.state === "Running" && lastAxis === axis && last !== null ? sweepProgress(tune, last) : undefined}
      />
      {indices.map((i) => (
        <div key={i} className="space-y-4">
          {!(announcing && i === last) && (
            <StepCard
              tune={tune}
              index={i}
              extensionText={actions.extension && i === last ? (extensionOffer(tune, i)?.text ?? null) : null}
            />
          )}
          {actions.decide && i === last && <PausePanel tune={tune} step={i} />}
        </div>
      ))}
      {announcing && last !== null && <AnnouncedStep tune={tune} step={tune.steps[last]} live={live} />}
      {preparing && prepared !== null && (
        <NextStepCard
          axis={axis}
          busy={busy}
          form={prepare}
          state={prepared}
          live={live}
          runs={runs}
          onForm={onPrepare}
        />
      )}
    </>
  );
}

/** A prepared step as it will run: its settings locked, and what running it does. */
function AnnouncedStep({
  tune,
  step,
  live,
}: {
  tune: TuneDetail;
  step: TuneDetail["steps"][number];
  live: number | null;
}) {
  const form = formFromStep(step, live);
  const control = { velocity_p: 0, velocity_i: 0, position_p: 0, pwm_hz: step.pwm_hz };
  const announcement = announcementOf(tune);
  return (
    <NextStepCard
      axis={step.axis}
      busy
      form={form}
      state={prepareState(step.axis, form, control)}
      live={live}
      onForm={() => undefined}
      runs={announcement?.runCount}
    />
  );
}

function LoadFailure({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div role="alert" className="space-y-2 text-sm">
      <p className="text-red-400">{message}</p>
      <Button onClick={onRetry}>Try again</Button>
    </div>
  );
}

function LockBanner({
  detail,
  busy,
  canRestore,
  onRestore,
}: {
  detail: TuneDetail;
  busy: boolean;
  canRestore: boolean;
  onRestore: () => void;
}) {
  const reason =
    detail.lock ??
    (typeof detail.state === "object"
      ? detail.state.RestoreFailed
      : "the starting gains have not been written back yet");
  return (
    <div role="alert" className="rounded-xl border border-amber-500/60 bg-card p-4 text-sm">
      <p className="font-medium text-amber-300">The stepper is locked until its starting gains are restored</p>
      <p className="mt-1 text-muted-foreground">{reason}</p>
      <div className="mt-3 flex gap-2">
        {canRestore && (
          <Button disabled={busy} onClick={onRestore}>
            Restore the starting gains
          </Button>
        )}
      </div>
    </div>
  );
}

function StepCard({ tune, index, extensionText }: { tune: Tune; index: number; extensionText: string | null }) {
  const step = tune.steps[index];
  const rows = rungView(tune, index);
  const summary = stepSummary(tune, index);
  const mark = recommendedGain(tune, index);
  const label = axisLabel(step.axis);

  return (
    <Card label={`${axisTitle(step.axis)} sweep`}>
      <p className="mb-3 text-sm">{stepSentence(step)}</p>
      {step.chosen_shown !== null && (
        <p className="mb-3 text-sm">
          Chosen gain:{" "}
          {step.axis === "VelocityICornerHz" ? `${step.chosen_shown.toFixed(1)} Hz` : Math.round(step.chosen_shown)}
        </p>
      )}
      {summary.failed !== null && <p className="mb-2 text-sm text-amber-400">Stopped: {summary.failed}</p>}
      {summary.halt !== null && <p className="mb-2 text-sm text-amber-400">{summary.halt}</p>}
      <SweepResultsTable axis={step.axis} label={label} rows={rows} speeds={tune.setup.speeds_mm_s} />
      {standstillCheckNote(tune, index) !== null && (
        <p className="mt-2 text-xs text-muted-foreground">{standstillCheckNote(tune, index)}</p>
      )}
      {driftCheckRunning(tune, index) && (
        <p className="mt-1 text-xs text-muted-foreground">
          Final check: running the starting gain once more, so the machine is still moving.
        </p>
      )}
      {summary.notes.length > 0 && (
        <ul className="mt-2 space-y-0.5 text-xs text-muted-foreground">
          {summary.notes.map((n) => (
            <li key={n}>{n}</li>
          ))}
        </ul>
      )}
      {rows.some((r) => r.p99BySpeed.some((v) => v !== null)) && (
        <div className="mt-3">
          <GuidedCurve rows={rows} speeds={tune.setup.speeds_mm_s} axisLabel={label} recommended={mark} />
        </div>
      )}
      {summary.driftLines.map((line) => (
        <p key={line} className="mt-1 text-xs text-muted-foreground">
          {line}
        </p>
      ))}

      {extensionText !== null && <p className="mt-4 border-t pt-4 text-sm">{extensionText}</p>}
    </Card>
  );
}
