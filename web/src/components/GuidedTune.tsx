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
  type EngineView,
  type PrinterView,
  type Tune,
  type TuneChoiceBody,
  type TuneDecision,
  type TuneDetail,
  type TuneId,
  type TuneStepPlan,
  type TuneSummary,
} from "@/api";
import { ArrowRightIcon } from "lucide-react";
import { Button as UiButton } from "@/components/ui/button";
import { failureMessage } from "@/failure";
import { useFeed } from "@/feed";
import {
  announcementOf,
  availableActions,
  axisLabel,
  axisOfStep,
  currentGuidedStep,
  currentStep,
  driftCheckRunning,
  durationText,
  expectedDurationSeconds,
  failureText,
  GUIDED_STEPS,
  nextAxis,
  recommendedGain,
  rungView,
  stepSummary,
  DEFAULT_PREPARE,
  liveControlFor,
  liveSeed,
  prepareState,
  type GuidedAction,
  type GuidedStepId,
  type PrepareForm,
  type PrepareState,
  type RungView,
} from "@/guided";
import { cn } from "@/lib/utils";
import { GuidedCurve } from "./GuidedCurve";
import { GuidedSetup } from "./GuidedSetup";
import { NextStepCard } from "./NextStepCard";
import { PausePanel } from "./PausePanel";
import { Steps } from "./Steps";
import { TuneRunCard } from "./TuneRunCard";
import { ActionBar } from "./WorkbenchCards";
import { Button, Card, Field, NumberField } from "./ui";

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

function useEarlierTunes(id: TuneId, enabled: boolean): Tune[] {
  const [earlier, setEarlier] = useState<Tune[]>([]);
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    getTunes()
      .then((list) =>
        Promise.all(
          list
            .filter((s) => s.id !== id)
            .sort((a, b) => b.created - a.created)
            .slice(0, 5)
            .map((s) => getTune(s.id)),
        ),
      )
      .then((tunes) => live && setEarlier(tunes))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [id, enabled]);
  return earlier;
}

function TuneView({ id, printer, offline }: { id: TuneId; printer: PrinterView; offline: boolean }) {
  const { detail, error, refresh } = useTune(id);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [plan, setPlan] = useState<TuneStepPlan | null>(null);
  const announced = detail?.state === "Announced";
  const earlier = useEarlierTunes(id, announced);
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
  const starting = announced && actions.start && axis !== null && lastAxis === axis;
  const liveControl = liveControlFor(printer, detail.setup.stepper);
  const prepared = preparing && next !== null ? prepareState(next, prepare, liveControl) : null;
  const prepareBlocked = prepared?.problem ?? null;
  const setupEditable = actions.prepare && detail.steps.length === 0;
  const abandon = actions.abandon ? (
    <Button disabled={frozen} onClick={() => run("abandon", () => postTuneAbandon(id))}>
      Abandon tune
    </Button>
  ) : null;

  return (
    <div className="space-y-4">
      <Steps steps={GUIDED_STEPS} current={shown} onSelect={(next) => setPicked(next as GuidedStepId)} />

      {detail.notes.length > 0 && (
        <ul className="space-y-0.5 text-xs text-muted-foreground">
          {detail.notes.map((n) => (
            <li key={n.unix_ms}>{n.text}</li>
          ))}
        </ul>
      )}

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
          plan={plan}
          earlier={earlier}
          engine={printer.engine}
          announced={announced}
          onChoose={(choice) => run("choose", () => postTuneChoice(id, choice))}
          onExtend={(direction) => run("extend", () => postTuneExtension(id, direction))}
          onDecide={(d) => run("decide", () => postTuneDecision(id, d))}
          prepare={prepare}
          prepared={prepared}
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
          {preparing && (
            <UiButton
              disabled={frozen || prepareBlocked !== null}
              onClick={() =>
                run("prepare", () =>
                  postTuneStep(id, {
                    seed: prepare.seed === null ? "live" : { typed: prepare.seed },
                    repeats: prepare.repeats,
                    lower_k: prepare.lower,
                    upper_k: prepare.upper,
                  }).then(setPlan),
                )
              }
            >
              Next
              <ArrowRightIcon />
            </UiButton>
          )}
          {starting && (
            <Button
              disabled={frozen}
              onClick={() =>
                run(
                  "start",
                  () => postTuneConfirm(id),
                  () => setPlan(null),
                )
              }
            >
              Start the step
            </Button>
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
  plan,
  earlier,
  engine,
  announced,
  onChoose,
  onExtend,
  onDecide,
  prepare,
  prepared,
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
  plan: TuneStepPlan | null;
  earlier: Tune[];
  engine: EngineView;
  announced: boolean;
  onChoose: (choice: TuneChoiceBody) => void;
  onExtend: (direction: "below" | "above") => void;
  onDecide: (d: TuneDecision) => void;
  prepare: PrepareForm;
  prepared: PrepareState | null;
  live: number | null;
  onPrepare: (form: PrepareForm) => void;
}) {
  const indices = tune.steps.flatMap((s, i) => (s.axis === axis ? [i] : []));
  const preparing = prepareAxis === axis;
  const announcing = announced && actions.start && lastAxis === axis;
  if (indices.length === 0 && !preparing) {
    return <p className="text-sm text-muted-foreground">{axisLabel(axis)} is not reached yet.</p>;
  }
  const sessions = lastAxis === axis && last !== null ? Object.values(tune.steps[last].sessions) : [];
  return (
    <>
      <TuneRunCard engine={engine} sessions={sessions} />
      {indices.map((i) => (
        <div key={i} className="space-y-4">
          <StepCard
            tune={tune}
            index={i}
            active={i === last}
            busy={busy}
            canChoose={actions.choose && i === last}
            canExtend={actions.extension && i === last}
            onChoose={onChoose}
            onExtend={onExtend}
          />
          {actions.decide && i === last && <PausePanel tune={tune} step={i} busy={busy} onDecide={onDecide} />}
        </div>
      ))}
      {announcing && <AnnouncementPanel tune={tune} plan={plan} earlier={earlier} />}
      {preparing && prepared !== null && (
        <NextStepCard axis={axis} busy={busy} form={prepare} state={prepared} live={live} onForm={onPrepare} />
      )}
    </>
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

function AnnouncementPanel({ tune, plan, earlier }: { tune: Tune; plan: TuneStepPlan | null; earlier: Tune[] }) {
  const a = announcementOf(tune);
  if (a === null) return null;
  const duration = durationText(expectedDurationSeconds([tune, ...earlier], a.runCount));
  return (
    <Card label="Announcement" className="border-emerald-500/60">
      <dl className="flex flex-wrap gap-x-8 gap-y-2">
        <Field label="Stepper">{a.stepper}</Field>
        <Field label="Axis">{axisLabel(a.axis)}</Field>
        <Field label="Rungs">
          {a.firstGain.toFixed(a.axis === "VelocityICornerHz" ? 1 : 0)} to{" "}
          {a.lastGain.toFixed(a.axis === "VelocityICornerHz" ? 1 : 0)}
        </Field>
        <Field label="Runs" sub={`${a.rungs} rungs, ${a.repeats} repeats`}>
          {a.runCount}
        </Field>
        <Field label="Expected duration">{duration}</Field>
      </dl>
      <p className="mt-3 text-sm font-medium">The machine will move once you start this step.</p>
      {plan !== null && <p className="mt-1 text-xs text-muted-foreground">{plan.announcement}</p>}
    </Card>
  );
}

const fmt = (n: number | null, digits = 3) => (n === null ? "-" : n.toFixed(digits));

function RungTable({ rows, speeds, axis }: { rows: RungView[]; speeds: number[]; axis: string }) {
  return (
    <div className="overflow-x-auto">
      <table aria-label={`${axis} rungs`} className="w-full text-left text-sm">
        <thead className="text-xs uppercase tracking-wider text-muted-foreground">
          <tr>
            <th className="pr-3">Rung</th>
            <th className="pr-3">Gain</th>
            <th className="pr-3">Raw</th>
            <th className="pr-3">Verdict</th>
            <th className="pr-3">Score (p99 μm)</th>
            <th className="pr-3">Behind by</th>
            <th className="pr-3">Zeta</th>
            {speeds.map((s) => (
              <th key={s} className="pr-3">
                p99 {s}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="font-mono">
          {rows.map((r) => (
            <tr
              key={r.index}
              className={cn(
                r.recommended !== null && "text-emerald-300",
                r.skipped !== null && "text-muted-foreground",
              )}
            >
              <td className="pr-3">{r.index + 1}</td>
              <td className="pr-3">{r.shown.toFixed(2)}</td>
              <td className="pr-3">{r.raw}</td>
              <td className="pr-3 font-sans">
                {r.verdict}
                {r.recommended !== null && <span className="ml-2 text-xs">({r.recommended})</span>}
              </td>
              <td className="pr-3">
                {fmt(r.score)}
                {r.decidingSpeed !== null && <span className="ml-1 text-xs"> @ {speeds[r.decidingSpeed]} mm/s</span>}
              </td>
              <td className="pr-3">
                {r.best
                  ? "best"
                  : r.gap === null
                    ? ""
                    : r.tolerance === null
                      ? `${fmt(r.gap, 1)} μm`
                      : `${fmt(r.gap, 1)} μm ± ${fmt(r.tolerance, 1)} μm`}
              </td>
              <td className="pr-3">{fmt(r.zeta)}</td>
              {speeds.map((s, i) => (
                <td key={s} className="pr-3">
                  {fmt(r.p99BySpeed[i] ?? null)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function StepCard({
  tune,
  index,
  active,
  busy,
  canChoose,
  canExtend,
  onChoose,
  onExtend,
}: {
  tune: Tune;
  index: number;
  active: boolean;
  busy: boolean;
  canChoose: boolean;
  canExtend: boolean;
  onChoose: (choice: TuneChoiceBody) => void;
  onExtend: (direction: "below" | "above") => void;
}) {
  const step = tune.steps[index];
  const rows = rungView(tune, index);
  const summary = stepSummary(tune, index);
  const mark = recommendedGain(tune, index);
  const label = axisLabel(step.axis);
  const measured = rows.filter((r) => r.skipped === null && r.repeatsDone >= step.repeats);
  const [rungPick, setRungPick] = useState<number | null>(null);
  const [typed, setTyped] = useState(mark?.gain ?? rows[0]?.shown ?? 0);
  const typedByOperator = useRef(false);
  const markGain = mark?.gain ?? null;
  useEffect(() => {
    if (markGain !== null && !typedByOperator.current) setTyped(markGain);
  }, [markGain]);
  const [extension, setExtension] = useState<"below" | "above" | null>(null);
  const rec = summary.recommendation;
  const pick = rungPick ?? measured[0]?.index ?? null;
  const perRepeat = tune.setup.speeds_mm_s.length + 2;

  return (
    <Card label={`Step ${index + 1}: ${label}`}>
      <dl className="mb-3 flex flex-wrap gap-x-8 gap-y-2">
        <Field label="Seed (raw)">{step.seed_raw}</Field>
        <Field label="Range">
          k {step.lower_k} to {step.upper_k}
        </Field>
        <Field label="Repeats">{step.repeats}</Field>
        <Field label="Chosen">{step.chosen_shown === null ? "not yet" : step.chosen_shown.toFixed(2)}</Field>
      </dl>
      {summary.failed !== null && <p className="mb-2 text-sm text-amber-400">Stopped: {summary.failed}</p>}
      {summary.halt !== null && <p className="mb-2 text-sm text-amber-400">{summary.halt}</p>}
      <RungTable rows={rows} speeds={tune.setup.speeds_mm_s} axis={label} />
      {driftCheckRunning(tune, index) && (
        <p className="mt-1 text-xs text-muted-foreground">
          Drift check running: the seed rung is repeated once more, so the machine is still moving.
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
      {rec !== null && (
        <p className="mt-2 text-sm">
          {rec.kind === "measured" && `Recommended: rung ${rec.rung + 1}, measured`}
          {rec.kind === "interpolated" &&
            `Recommended: ${rec.shown.toFixed(2)}, interpolated between rungs ${rec.between[0] + 1} and ${rec.between[1] + 1}`}
          {rec.kind === "none" && "No rung scored, so there is no recommendation"}
        </p>
      )}
      {summary.comparison !== null && <p className="mt-1 text-sm">{summary.comparison}</p>}
      {summary.driftLines.map((line) => (
        <p key={line} className="mt-1 text-xs text-muted-foreground">
          {line}
        </p>
      ))}
      {step.drift.length > 0 && (
        <p className="mt-1 text-xs text-muted-foreground">
          Drift of the seed rung: {step.drift.map((d) => `${d.metric} ${fmt(d.first)} to ${fmt(d.rerun)}`).join("; ")}
        </p>
      )}

      {active && canChoose && (
        <div className="mt-4 space-y-3 border-t pt-4">
          <div className="flex flex-wrap items-end gap-3">
            <Button disabled={busy || rec === null || rec.kind === "none"} onClick={() => onChoose("recommended")}>
              Accept the recommendation
            </Button>
            <label className="flex flex-col text-xs text-muted-foreground">
              Rung
              <select
                value={pick ?? ""}
                disabled={busy || measured.length === 0}
                onChange={(e) => setRungPick(Number(e.target.value))}
                className="rounded border bg-background px-1.5 py-1 text-sm text-foreground"
              >
                {measured.map((r) => (
                  <option key={r.index} value={r.index}>
                    {r.index + 1}: {r.shown.toFixed(2)}
                  </option>
                ))}
              </select>
            </label>
            <Button disabled={busy || pick === null} onClick={() => pick !== null && onChoose({ rung: pick })}>
              Use this rung
            </Button>
            <label className="flex flex-col text-xs text-muted-foreground">
              Value
              <NumberField
                label="Value"
                value={typed}
                disabled={busy}
                onValue={(v) => {
                  typedByOperator.current = true;
                  setTyped(v);
                }}
                className="w-28"
              />
            </label>
            <Button disabled={busy || !(typed > 0)} onClick={() => onChoose({ value: typed })}>
              Use this value
            </Button>
          </div>
        </div>
      )}

      {active && canExtend && summary.extend !== null && (
        <div className="mt-3 space-y-2 border-t pt-3">
          <p className="text-sm">
            The best rung is at the {summary.extend === "Below" ? "low" : "high"} end of the ladder. Extending adds one
            rung {summary.extend === "Below" ? "below" : "above"} it.
          </p>
          {extension === null ? (
            <Button disabled={busy} onClick={() => setExtension(summary.extend === "Below" ? "below" : "above")}>
              Extend the ladder {summary.extend === "Below" ? "below" : "above"}
            </Button>
          ) : (
            <div className="space-y-2">
              <p className="text-sm font-medium">
                This runs {step.repeats * perRepeat} more runs and the machine will move.
              </p>
              <div className="flex gap-2">
                <Button
                  disabled={busy}
                  onClick={() => {
                    onExtend(extension);
                    setExtension(null);
                  }}
                >
                  Confirm the extension
                </Button>
                <Button disabled={busy} onClick={() => setExtension(null)}>
                  Cancel
                </Button>
              </div>
            </div>
          )}
        </div>
      )}
    </Card>
  );
}
