import type { ProbeEdgeHint } from "@/tune";

const NOTE_CLASS = {
  caution: "rounded border border-amber-500/60 px-2 py-1 text-xs text-amber-300",
  warning: "rounded border border-red-500/60 px-2 py-1 text-xs text-red-300",
} as const;

export function EdgeHintLine({ hint }: { hint: ProbeEdgeHint }) {
  if (hint.tone === "ok") return null;
  return (
    <p role="note" className={NOTE_CLASS[hint.tone]}>
      {hint.text}
    </p>
  );
}

/** The notes about the probe's gain-change spacing and step responses; a spacing problem comes first and alone. */
export function ProbeNotes({ spacingProblem, hint }: { spacingProblem: string | null; hint: ProbeEdgeHint | null }) {
  if (spacingProblem !== null) {
    return (
      <p role="note" className={NOTE_CLASS.caution}>
        {spacingProblem}
      </p>
    );
  }
  return hint === null ? null : <EdgeHintLine hint={hint} />;
}
