import type { ProbeEdgeHint } from "@/tune";

const NOTE_CLASS = {
  caution: "rounded border border-amber-500/60 px-2 py-1 text-xs text-amber-300",
  warning: "rounded border border-red-500/60 px-2 py-1 text-xs text-red-300",
} as const;

/** The probe's expected step responses, shown only when the count is too low. */
export function EdgeHintLine({ hint }: { hint: ProbeEdgeHint }) {
  if (hint.tone === "ok") return null;
  return (
    <p role="note" className={NOTE_CLASS[hint.tone]}>
      {hint.text}
    </p>
  );
}
