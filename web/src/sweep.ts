import type { PrinterView, SessionView } from "./api";
import { finishedMs } from "./components/ResultsTable";
import {
  headlineMetric,
  ladderMedians,
  runTypeOfKind,
  sweepOrder,
  sweepRejectionNotes,
  TERMINAL_STATES,
  type RunType,
} from "./tune";

export function sweepLadderEntries(runs: SessionView["runs"], type: RunType) {
  return [...runs]
    .filter(
      (e) => e.run !== null && e.run.request.ladder_value !== null && e.state !== "skipped" && e.state !== "aborted",
    )
    .sort((a, b) => finishedMs(a) - finishedMs(b))
    .map((e) => ({
      value: e.run?.request.ladder_value ?? null,
      metric: e.run === null ? null : headlineMetric(e.run, type).value,
      assessable: e.state === "done",
    }));
}

/**
 * What happened to a probe sweep's runs, in plain words: why a value has no result and which
 * values did not run because the sweep stopped. Empty for a healthy sweep or no sweep.
 */
export function sweepNotesFor(view: SessionView, printer: PrinterView | null): string[] {
  const session = view.session;
  const ladder = session?.ladder ?? null;
  if (session === null || ladder === null) return [];
  const type = runTypeOfKind(session.kind);
  if (type !== "probe") return [];
  const entries = sweepOrder(
    view.runs.filter((r) => TERMINAL_STATES.has(r.state)).sort((a, b) => finishedMs(a) - finishedMs(b)),
    ladder.values,
  );
  const repeats = ladder.repeats;
  const unassessable = ladderMedians(sweepLadderEntries(entries, type), repeats)
    .filter((m) => m.unassessable)
    .map((m) => m.value);
  const tested = session.stepper;
  const board = printer?.steppers.find((s) => s.name === tested)?.serial;
  const others = (printer?.steppers ?? []).filter((s) => s.serial === board && s.name !== tested).map((s) => s.name);
  const place = (v: number) => ladder.values.indexOf(v);
  const rejected = [...unassessable]
    .sort((a, b) => place(a) - place(b))
    .flatMap((value) => {
      const runs = entries.filter((e) => e.run?.request.ladder_value === value).slice(0, repeats);
      const bad = runs.find(
        (e) => e.state !== "done" || (e.run !== null && headlineMetric(e.run, type).value === null),
      );
      if (bad === undefined) return [];
      return [
        {
          value,
          state: bad.state,
          reason: "reason" in bad ? String(bad.reason) : null,
          failedChecks: (bad.run?.verification ?? []).filter((v) => !v.passed).map((v) => v.check),
        },
      ];
    });
  const skippedValues = [
    ...new Set(
      entries.flatMap((e) => {
        const value = e.run?.request.ladder_value;
        return e.state === "skipped" && value != null ? [value] : [];
      }),
    ),
  ].sort((a, b) => place(a) - place(b));
  return sweepRejectionNotes({
    rejected,
    skippedValues,
    unit: ladder.axis === "velocity_i_corner_hz" ? "Hz" : "",
    tested,
    others,
  });
}
