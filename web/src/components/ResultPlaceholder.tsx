import { Skeleton } from "@/components/ui/skeleton";
import { Card } from "./ui";

/** What the Result step shows until every axis has a chosen gain: what to do, and the shape of the result to come. */
export function ResultPlaceholder({ chosen }: { chosen: number }) {
  return (
    <Card label="Result">
      <div role="status">
        <p className="text-sm">Choose a gain for each of the three steps, and the result appears here.</p>
        <p className="mt-1 text-xs text-muted-foreground">{chosen} of 3 gains chosen.</p>
      </div>
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
