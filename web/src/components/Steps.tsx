import { CheckIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export interface Step {
  id: string;
  label: string;
}

export function Steps({
  steps,
  current,
  onSelect,
}: {
  steps: Step[];
  current: string;
  onSelect: (id: string) => void;
}) {
  const at = steps.findIndex((s) => s.id === current);
  return (
    <nav aria-label="Steps">
      <ol
        className="grid items-center gap-3"
        style={{ gridTemplateColumns: `repeat(${steps.length}, minmax(0, 1fr))` }}
      >
        {steps.map((step, index) => {
          const state = at < 0 ? "upcoming" : index < at ? "done" : index === at ? "current" : "upcoming";
          return (
            <li key={step.id} className="flex items-center gap-3">
              <button
                type="button"
                data-state={state}
                aria-current={state === "current" ? "step" : undefined}
                onClick={() => onSelect(step.id)}
                className="group flex shrink-0 items-center gap-2 rounded-md text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                <span
                  className={cn(
                    "box-border flex size-7 shrink-0 items-center justify-center rounded-full border-2 text-xs font-medium",
                    state === "current" && "border-emerald-500 bg-emerald-600 text-white",
                    state === "done" && "border-emerald-500 bg-card text-emerald-400",
                    state === "upcoming" && "border-border bg-card text-muted-foreground",
                  )}
                >
                  {state === "done" ? <CheckIcon aria-hidden="true" className="size-4" /> : null}
                  {state === "done" ? <span className="sr-only">{index + 1}</span> : index + 1}
                </span>
                <span
                  className={cn(
                    state === "current" ? "font-medium" : "text-muted-foreground group-hover:text-foreground",
                  )}
                >
                  {step.label}
                </span>
              </button>
              {index < steps.length - 1 && (
                <span
                  aria-hidden="true"
                  className={cn("h-px flex-1", state === "done" ? "bg-emerald-500" : "bg-border")}
                />
              )}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
