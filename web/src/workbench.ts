import type { Filters, Pattern } from "./api";
import {
  DEFAULT_PARAMETERS,
  type LadderInput,
  type OuterGains,
  type PatternMode,
  type RunType,
  type SessionLink,
} from "./tune";

/** The Tune page's form and session link, kept outside React so a route change keeps them. */
export interface WorkbenchState {
  stepperName: string;
  type: RunType;
  params: typeof DEFAULT_PARAMETERS;
  speedsText: string;
  patterns: Record<string, Pattern>;
  patternModes: Record<string, PatternMode>;
  gains: OuterGains | null;
  filters: Filters | null;
  seededFor: string | null;
  ladderOn: boolean;
  ladder: LadderInput;
  link: SessionLink | null;
}

export const workbench: Partial<WorkbenchState> = {};

export function resetWorkbench(): void {
  for (const key of Object.keys(workbench) as (keyof WorkbenchState)[]) delete workbench[key];
}
