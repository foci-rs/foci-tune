import type { ReactNode } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export function Explained({ tip, children }: { tip: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span tabIndex={0} className="cursor-help underline decoration-dotted">
          {children}
        </span>
      </TooltipTrigger>
      <TooltipContent className="max-w-xs">{tip}</TooltipContent>
    </Tooltip>
  );
}
