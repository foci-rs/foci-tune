import { CheckCircle2Icon, CircleIcon } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import { Card } from "./ui";

/** What the Result step shows until every axis has a chosen gain: which are chosen, and the shape of the result to come. */
export function ResultPlaceholder({ items }: { items: { label: string; done: boolean }[] }) {
  return (
    <Card label="Result">
      <ul aria-label="Gains chosen" className="space-y-2 text-sm">
        {items.map((item) => (
          <li key={item.label} data-done={item.done} className="flex items-center gap-2">
            {item.done ? (
              <CheckCircle2Icon aria-hidden="true" className="size-4 text-emerald-400" />
            ) : (
              <CircleIcon aria-hidden="true" className="size-4 text-muted-foreground" />
            )}
            <span className={item.done ? "" : "text-muted-foreground"}>{item.label}</span>
            <span className="sr-only">: {item.done ? "chosen" : "not chosen yet"}</span>
          </li>
        ))}
      </ul>
      <div aria-hidden="true" className="mt-5 space-y-3">
        {[0, 1, 2].map((row) => (
          <div key={row} className="flex items-center gap-6">
            <Skeleton className="h-4 w-32" />
            <Skeleton className="h-4 w-16" />
            <Skeleton className="h-4 w-16" />
            <Skeleton className="h-4 w-12" />
          </div>
        ))}
        <Skeleton className="mt-4 h-28 w-full" />
      </div>
    </Card>
  );
}
