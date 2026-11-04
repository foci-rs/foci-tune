import { useEffect, useState } from "react";
import { getSession, type EngineView, type Run, type SessionId } from "@/api";
import { useNow } from "@/useNow";
import { RunQueueCard } from "./RunQueueCard";

/** The run in flight, when it belongs to one of the given sessions; nothing otherwise. */
export function TuneRunCard({
  engine,
  sessions,
  sweep,
}: {
  engine: EngineView;
  sessions: SessionId[];
  /** The whole step's runs, shown even between two runs while the step is running. */
  sweep?: { done: number; total: number };
}) {
  const current = engine.current !== null && sessions.includes(engine.current.session) ? engine.current : null;
  const [record, setRecord] = useState<Run | null>(null);
  const run = current?.run;
  const state = current?.state;
  const session = current?.session;
  useEffect(() => {
    if (session === undefined) {
      setRecord(null);
      return;
    }
    let live = true;
    getSession(session).then(
      (view) => live && setRecord(view.runs.find((r) => r.id === run)?.run ?? null),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [session, run, state]);
  const nowMs = useNow(current !== null);
  if (current === null && sweep === undefined) return null;
  return (
    <RunQueueCard
      current={current}
      otherSession={false}
      position={null}
      steps={record?.steps ?? []}
      predictedS={record?.predicted_s ?? null}
      nowMs={nowMs}
      pending={[]}
      queued={engine.queue.length}
      sweep={sweep}
      showQueue={false}
    />
  );
}
