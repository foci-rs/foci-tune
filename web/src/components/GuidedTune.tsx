import { useCallback, useEffect, useRef, useState } from "react";
import {
  ApiRequestError,
  getTune,
  getTunes,
  postTuneAbandon,
  postTuneAcknowledge,
  postTuneChoice,
  postTuneConfirm,
  postTuneExtension,
  postTuneRestore,
  postTuneStep,
  type PrinterView,
  type Tune,
  type TuneChoiceBody,
  type TuneDetail,
  type TuneId,
  type TuneStepPlan,
  type TuneSummary,
} from "@/api";
import { useFeed } from "@/feed";
import {
  announcementOf,
  availableActions,
  axisLabel,
  currentStep,
  durationText,
  expectedDurationSeconds,
  nextAxis,
  recommendedGain,
  rungView,
  stateText,
  stepSummary,
  type RungView,
} from "@/guided";
import { cn } from "@/lib/utils";
import { GuidedCurve } from "./GuidedCurve";
import { GuidedSetup } from "./GuidedSetup";
import { Button, Card, Field, NumberField } from "./ui";

function errorText(e: unknown): string {
  if (e instanceof ApiRequestError) return e.field === null ? e.message : `${e.field}: ${e.message}`;
  return e instanceof Error ? e.message : "request failed";
}

const pillClass = (active: boolean) =>
  active
    ? "flex items-center gap-2 rounded-full border border-emerald-500 px-3 py-1 text-sm text-emerald-300"
    : "flex items-center gap-2 rounded-full border px-3 py-1 text-sm text-muted-foreground";

const isOver = (s: TuneSummary) => s.state === "Done" || s.state === "Abandoned";

/** The guided tune: one tune at a time, step by step, with the operator confirming every motion. */
export function GuidedTune({ printer }: { printer: PrinterView }) {
  const { listen } = useFeed();
  const [summaries, setSummaries] = useState<TuneSummary[] | null>(null);
  const [selected, setSelected] = useState<TuneId | "new" | null>(null);
  const [listError, setListError] = useState<string | null>(null);

  const load = useCallback(() => {
    getTunes().then(
      (list) => {
        setSummaries([...list].sort((a, b) => b.created - a.created));
        setListError(null);
      },
      (e: unknown) => setListError(errorText(e)),
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
    return <p className="text-sm text-muted-foreground">{listError ?? "Loading tunes"}</p>;
  }
  const open = summaries.filter((s) => !isOver(s));
  const current = selected ?? open[0]?.id ?? "new";

  return (
    <div className="space-y-4">
      <nav aria-label="Tunes" className="flex flex-wrap items-center gap-2">
        {summaries.map((s) => (
          <button key={s.id} type="button" onClick={() => setSelected(s.id)} className={pillClass(current === s.id)}>
            {s.stepper}
            <span className="font-mono text-xs text-muted-foreground">{stateText(s.state).split(":")[0]}</span>
          </button>
        ))}
        <button type="button" onClick={() => setSelected("new")} className={pillClass(current === "new")}>
          New tune
          {open.length > 0 && (
            <span aria-label={`${open.length} unfinished`} className="rounded-full bg-accent px-1.5 text-xs">
              {open.length}
            </span>
          )}
        </button>
      </nav>
      {listError !== null && <p className="text-sm text-red-400">{listError}</p>}
      {current === "new" ? (
        <GuidedSetup
          printer={printer}
          onCreated={(tune) => {
            load();
            setSelected(tune.id);
          }}
        />
      ) : (
        <TuneView key={current} id={current} />
      )}
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
      (e: unknown) => mine === sequence.current && setError(errorText(e)),
    );
  }, [id]);
  useEffect(refresh, [refresh]);
  useEffect(
    () =>
      listen((m) => {
        if (m.type === "tune" && m.tune.id === id) refresh();
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
            .filter((s) => s.id !== id && s.state === "Done")
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

function TuneView({ id }: { id: TuneId }) {
  const { detail, error, refresh } = useTune(id);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [plan, setPlan] = useState<TuneStepPlan | null>(null);
  const announced = detail?.state === "Announced";
  const earlier = useEarlierTunes(id, announced);

  const run = (action: () => Promise<unknown>, after?: () => void) => {
    setBusy(true);
    setActionError(null);
    action()
      .then(() => after?.())
      .catch((e: unknown) => setActionError(errorText(e)))
      .finally(() => {
        setBusy(false);
        refresh();
      });
  };

  if (detail === null) return <p className="text-sm text-muted-foreground">{error ?? "Loading the tune"}</p>;
  const actions = availableActions(detail, detail.lock);
  const last = currentStep(detail);

  return (
    <div className="space-y-4">
      <Card label="Tune">
        <dl className="flex flex-wrap gap-x-8 gap-y-2">
          <Field label="Stepper">{detail.setup.stepper}</Field>
          <Field label="State">{stateText(detail.state)}</Field>
          <Field label="Tracking speeds">{detail.setup.speeds_mm_s.join(", ")} mm/s</Field>
          <Field label="Steps">{detail.steps.length} of 3</Field>
        </dl>
        {detail.notes.length > 0 && (
          <ul className="mt-2 space-y-0.5 text-xs text-muted-foreground">
            {detail.notes.map((n) => (
              <li key={n.unix_ms}>{n.text}</li>
            ))}
          </ul>
        )}
        {actions.abandon && (
          <Button className="mt-3" disabled={busy} onClick={() => run(() => postTuneAbandon(id))}>
            Abandon tune
          </Button>
        )}
      </Card>

      {(actions.restore || actions.acknowledge) && (
        <LockBanner
          detail={detail}
          busy={busy}
          canRestore={actions.restore}
          canAcknowledge={actions.acknowledge}
          onRestore={() => run(() => postTuneRestore(id))}
          onAcknowledge={() => run(() => postTuneAcknowledge(id))}
        />
      )}

      {actionError !== null && (
        <p role="alert" className="text-sm text-red-400">
          {actionError}
        </p>
      )}

      {detail.steps.map((_, i) => (
        <StepCard
          key={i}
          tune={detail}
          index={i}
          active={i === last}
          busy={busy}
          canChoose={actions.choose && i === last}
          canExtend={actions.extension && i === last}
          onChoose={(choice) => run(() => postTuneChoice(id, choice))}
          onExtend={(direction) => run(() => postTuneExtension(id, direction))}
        />
      ))}

      {announced && actions.start && (
        <AnnouncementPanel
          tune={detail}
          plan={plan}
          earlier={earlier}
          busy={busy}
          onStart={() => run(() => postTuneConfirm(id), () => setPlan(null))}
        />
      )}

      {actions.prepare && nextAxis(detail) !== null && (
        <PrepareCard
          axis={nextAxis(detail) as NonNullable<ReturnType<typeof nextAxis>>}
          busy={busy}
          onPrepare={(body) => run(() => postTuneStep(id, body).then(setPlan))}
        />
      )}

      {detail.config_block !== null && (
        <Card label="Config block">
          <pre className="overflow-x-auto font-mono text-xs">{detail.config_block}</pre>
        </Card>
      )}
    </div>
  );
}

function LockBanner({
  detail,
  busy,
  canRestore,
  canAcknowledge,
  onRestore,
  onAcknowledge,
}: {
  detail: TuneDetail;
  busy: boolean;
  canRestore: boolean;
  canAcknowledge: boolean;
  onRestore: () => void;
  onAcknowledge: () => void;
}) {
  const reason =
    detail.lock ?? (typeof detail.state === "object" ? detail.state.RestoreFailed : "the starting gains have not been written back yet");
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
        {canAcknowledge && (
          <Button disabled={busy} onClick={onAcknowledge}>
            Acknowledge
          </Button>
        )}
      </div>
    </div>
  );
}

function PrepareCard({
  axis,
  busy,
  onPrepare,
}: {
  axis: NonNullable<ReturnType<typeof nextAxis>>;
  busy: boolean;
  onPrepare: (body: { seed: "live" | { typed: number }; repeats: number; lower_k: number; upper_k: number }) => void;
}) {
  const [typed, setTyped] = useState(false);
  const [seed, setSeed] = useState(0);
  const [repeats, setRepeats] = useState(3);
  const [lower, setLower] = useState(-3);
  const [upper, setUpper] = useState(3);
  const problem =
    repeats < 1 || !Number.isInteger(repeats)
      ? "repeats must be a whole number of at least one"
      : !Number.isInteger(lower) || !Number.isInteger(upper) || lower > upper
        ? "the range must be whole exponents with the lower at most the upper"
        : typed && !(seed > 0)
          ? "the seed must be above zero"
          : null;
  return (
    <Card label={`Next step: ${axisLabel(axis)}`}>
      <div className="flex flex-wrap items-end gap-3">
        <div role="radiogroup" aria-label="Seed" className="flex gap-1">
          <button type="button" role="radio" aria-checked={!typed} disabled={busy} onClick={() => setTyped(false)} className={pillClass(!typed)}>
            Live value
          </button>
          <button type="button" role="radio" aria-checked={typed} disabled={busy} onClick={() => setTyped(true)} className={pillClass(typed)}>
            Typed value
          </button>
        </div>
        {typed && (
          <label className="flex flex-col text-xs text-muted-foreground">
            Seed
            <NumberField label="Seed" value={seed} disabled={busy} onValue={setSeed} />
          </label>
        )}
        <label className="flex flex-col text-xs text-muted-foreground">
          Repeats
          <NumberField label="Repeats" value={repeats} disabled={busy} onValue={setRepeats} />
        </label>
        <label className="flex flex-col text-xs text-muted-foreground">
          Lowest rung k
          <NumberField label="Lowest rung k" value={lower} disabled={busy} onValue={setLower} />
        </label>
        <label className="flex flex-col text-xs text-muted-foreground">
          Highest rung k
          <NumberField label="Highest rung k" value={upper} disabled={busy} onValue={setUpper} />
        </label>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        Rungs are the seed times 2^(k/3). Preparing a step announces it; nothing moves until you start it.
      </p>
      {problem !== null && <p className="mt-1 text-xs text-amber-400">{problem}</p>}
      <Button
        className="mt-3"
        disabled={busy || problem !== null}
        onClick={() => onPrepare({ seed: typed ? { typed: seed } : "live", repeats, lower_k: lower, upper_k: upper })}
      >
        Prepare the step
      </Button>
    </Card>
  );
}

function AnnouncementPanel({
  tune,
  plan,
  earlier,
  busy,
  onStart,
}: {
  tune: Tune;
  plan: TuneStepPlan | null;
  earlier: Tune[];
  busy: boolean;
  onStart: () => void;
}) {
  const a = announcementOf(tune);
  if (a === null) return null;
  const duration = durationText(expectedDurationSeconds(earlier, a.runCount));
  return (
    <Card label="Announcement" className="border-emerald-500/60">
      <dl className="flex flex-wrap gap-x-8 gap-y-2">
        <Field label="Stepper">{a.stepper}</Field>
        <Field label="Axis">{axisLabel(a.axis)}</Field>
        <Field label="Rungs">
          {a.firstGain.toFixed(a.axis === "VelocityICornerHz" ? 1 : 0)} to {a.lastGain.toFixed(a.axis === "VelocityICornerHz" ? 1 : 0)}
        </Field>
        <Field label="Runs" sub={`${a.rungs} rungs, ${a.repeats} repeats`}>
          {a.runCount}
        </Field>
        <Field label="Expected duration">{duration}</Field>
      </dl>
      <p className="mt-3 text-sm font-medium">The machine will move once you start this step.</p>
      {plan !== null && <p className="mt-1 text-xs text-muted-foreground">{plan.announcement}</p>}
      <Button className="mt-3" disabled={busy} onClick={onStart}>
        Start the step
      </Button>
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
            <th className="pr-3">Score</th>
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
            <tr key={r.index} className={cn(r.recommended !== null && "text-emerald-300", r.skipped !== null && "text-muted-foreground")}>
              <td className="pr-3">{r.index + 1}</td>
              <td className="pr-3">{r.shown.toFixed(2)}</td>
              <td className="pr-3">{r.raw}</td>
              <td className="pr-3 font-sans">
                {r.verdict}
                {r.recommended !== null && <span className="ml-2 text-xs">({r.recommended})</span>}
              </td>
              <td className="pr-3">{fmt(r.score)}</td>
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
      {step.drift.length > 0 && (
        <p className="mt-1 text-xs text-muted-foreground">
          Drift of the seed rung:{" "}
          {step.drift.map((d) => `${d.metric} ${fmt(d.first)} to ${fmt(d.rerun)}`).join("; ")}
        </p>
      )}

      {active && canChoose && (
        <div className="mt-4 space-y-3 border-t pt-4">
          <div className="flex flex-wrap items-end gap-3">
            <Button
              disabled={busy || rec === null || rec.kind === "none"}
              onClick={() => onChoose("recommended")}
            >
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
            The best rung is at the {summary.extend === "Below" ? "low" : "high"} end of the ladder. Extending adds one rung{" "}
            {summary.extend === "Below" ? "below" : "above"} it.
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
