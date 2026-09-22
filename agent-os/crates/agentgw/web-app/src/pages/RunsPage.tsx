import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  cancelRun,
  decodeDataUri,
  getDecisions,
  getEnvironment,
  getRuns,
  type Decision,
  type Environment,
  type Run,
} from "@/lib/api";
import { cn } from "@/lib/utils";

const STATE_VARIANT: Record<string, "default" | "secondary" | "destructive" | "outline"> = {
  completed: "default",
  failed: "destructive",
  cancelled: "outline",
};

export function RunsPage({
  focusRunId,
  onConsumed,
}: {
  focusRunId: string | null;
  onConsumed: () => void;
}) {
  const [runs, setRuns] = useState<Run[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // Derive the row from the fresh list so the detail pane tracks state
  // transitions as polling refreshes `runs`.
  const selected = runs.find((r) => r.run_id === selectedId) ?? null;
  const [decisions, setDecisions] = useState<Decision[]>([]);
  const [env, setEnv] = useState<Environment | null>(null);
  const [envErr, setEnvErr] = useState("");

  const refresh = useCallback(async () => {
    try {
      const list = await getRuns();
      setRuns(list.slice().reverse());
    } catch (e) {
      toast.error((e as Error).message);
    }
  }, []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 3000);
    return () => clearInterval(t);
  }, [refresh]);

  useEffect(() => {
    if (focusRunId) {
      const r = runs.find((x) => x.run_id === focusRunId);
      if (r) {
        setSelectedId(r.run_id);
        onConsumed();
      }
    }
  }, [focusRunId, runs, onConsumed]);

  useEffect(() => {
    if (!selectedId) return;
    let live = true;
    const load = async () => {
      try {
        const [d, e] = await Promise.all([
          getDecisions(selectedId),
          getEnvironment(selectedId).catch((err) => {
            if (live) setEnvErr(err.message);
            return null;
          }),
        ]);
        if (!live) return;
        setDecisions(d);
        setEnv(e);
      } catch (err) {
        if (live) toast.error((err as Error).message);
      }
    };
    load();
    const t = setInterval(load, 3000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, [selectedId]);

  const out = selected ? decodeDataUri(selected.output_ref) : null;

  return (
    <div className="flex h-full">
      <div className="flex-1 overflow-auto p-6">
        <h1 className="mb-4 text-lg font-semibold">Runs</h1>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Run</TableHead>
              <TableHead>State</TableHead>
              <TableHead>Epoch</TableHead>
              <TableHead>Step</TableHead>
              <TableHead>Output</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {runs.map((r) => (
              <TableRow
                key={r.run_id}
                className={cn(
                  "cursor-pointer",
                  selected?.run_id === r.run_id && "bg-accent/50",
                )}
                onClick={() => {
                  setSelectedId(r.run_id);
                  setEnv(null);
                  setEnvErr("");
                }}
              >
                <TableCell className="font-mono text-xs">
                  {r.run_id.slice(0, 18)}…
                </TableCell>
                <TableCell>
                  <Badge variant={STATE_VARIANT[r.state_name] || "secondary"}>
                    {r.state_name}
                  </Badge>
                </TableCell>
                <TableCell>{r.loop_epoch}</TableCell>
                <TableCell>{r.step_sequence}</TableCell>
                <TableCell className="max-w-64 truncate text-muted-foreground">
                  {decodeDataUri(r.output_ref) || r.output_ref || "—"}
                </TableCell>
              </TableRow>
            ))}
            {!runs.length && (
              <TableRow>
                <TableCell colSpan={5} className="text-muted-foreground">
                  no runs yet — start one from Chat
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </div>

      {selected && (
        <div className="w-96 space-y-4 overflow-auto border-l p-5">
          <div className="flex items-center justify-between">
            <h2 className="font-medium">Run detail</h2>
            <Badge variant={STATE_VARIANT[selected.state_name] || "secondary"}>
              {selected.state_name}
            </Badge>
          </div>
          <div className="space-y-1 text-xs">
            <Row k="run" v={selected.run_id} mono />
            <Row k="task" v={selected.task_id} mono />
            <Row k="session" v={selected.session_id} mono />
            <Row
              k="revision"
              v={`${selected.run_revision} · epoch ${selected.loop_epoch} · step ${selected.step_sequence}`}
            />
          </div>
          {out && (
            <div>
              <div className="mb-1 text-xs font-medium text-muted-foreground">
                output
              </div>
              <div className="rounded-md bg-muted p-3 text-sm whitespace-pre-wrap">
                {out}
              </div>
            </div>
          )}
          <div>
            <div className="mb-1 text-xs font-medium text-muted-foreground">
              decisions
            </div>
            <div className="space-y-1.5">
              {decisions.map((d) => (
                <div
                  key={d.sequence}
                  className="rounded-md border p-2 text-xs"
                >
                  <div className="flex items-center gap-2">
                    <span className="text-muted-foreground">
                      step {d.step_sequence}
                    </span>
                    <Badge variant="outline">{d.kind}</Badge>
                  </div>
                  <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap text-[11px] text-muted-foreground">
                    {JSON.stringify(d.detail, null, 1)}
                  </pre>
                </div>
              ))}
              {!decisions.length && (
                <p className="text-xs text-muted-foreground">none yet</p>
              )}
            </div>
          </div>
          <div>
            <div className="mb-1 text-xs font-medium text-muted-foreground">
              environment
            </div>
            {env ? (
              <div className="space-y-1 rounded-md border p-2 text-xs">
                <Row k="profile env" v={env.environment.environment_id} mono />
                <Row
                  k="loop"
                  v={`${env.environment.agent_loop_id}@${env.environment.agent_loop_version}`}
                  mono
                />
                <Row k="generation" v={env.environment.config_generation_id} mono />
                <Row
                  k="model"
                  v={
                    [env.environment.model_provider, env.environment.model_id]
                      .filter(Boolean)
                      .join(" / ") || "—"
                  }
                />
                {env.bindings.map((b) => (
                  <Row
                    key={b.port_id}
                    k={b.port_id}
                    v={`${b.adapter_id}@${b.adapter_version}`}
                    mono
                  />
                ))}
              </div>
            ) : (
              <p className="text-xs text-muted-foreground">
                {envErr || "loading…"}
              </p>
            )}
          </div>
          {selected.state < 9 && (
            <Button
              variant="destructive"
              size="sm"
              onClick={() =>
                cancelRun(selected.run_id, "cancelled via gui")
                  .then(() => toast.success("cancel requested"))
                  .catch((e) => toast.error(e.message))
              }
            >
              Cancel run
            </Button>
          )}
        </div>
      )}
    </div>
  );
}

function Row({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div className="flex justify-between gap-2">
      <span className="text-muted-foreground">{k}</span>
      <span className={cn("truncate text-right", mono && "font-mono")}>{v}</span>
    </div>
  );
}
