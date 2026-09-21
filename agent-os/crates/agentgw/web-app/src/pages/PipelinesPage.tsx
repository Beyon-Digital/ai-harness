import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Background,
  Controls,
  Handle,
  Position,
  ReactFlow,
  addEdge,
  useEdgesState,
  useNodesState,
  type Connection,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { toast } from "sonner";
import { Boxes, GitBranch, Plus, Save, Upload } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import {
  getAdapters,
  getConfig,
  getProfiles,
  proposeConfig,
  type Adapter,
  type Profile,
} from "@/lib/api";

/* Profile binding slots the kernel resolves (config yaml keys). */
const SLOTS = [
  { key: "agent_loop", label: "agent_loop", adapterPort: "agent_loop" },
  { key: "effect_execute", label: "effect.execute", adapterPort: "effect.execute" },
] as const;

const BUILTIN_SLOTS = [
  "sandbox",
  "workspace",
  "artifact_store",
] as const;

type AdapterData = { adapter: Adapter };
type ProfileData = { name: string };

function AdapterNode({ data }: NodeProps<Node<AdapterData>>) {
  const a = data.adapter;
  return (
    <div className="w-64 rounded-lg border bg-card p-3 shadow-sm">
      <div className="flex items-center gap-2">
        <Boxes className="size-4 text-muted-foreground" />
        <div className="truncate font-mono text-xs">
          {a.adapter?.id.slice(0, 18)}…
        </div>
        <Badge variant="outline" className="ml-auto">
          {a.runtime_type}
        </Badge>
      </div>
      <div className="mt-1 text-xs text-muted-foreground">
        v{a.adapter?.version} · {a.trust_state}
      </div>
      {(a.ports || []).map((p) => (
        <div
          key={p}
          className="relative mt-1.5 rounded bg-muted px-2 py-1 font-mono text-[11px]"
        >
          {p}
          <Handle
            type="source"
            position={Position.Right}
            id={p}
            className="!size-2.5 !bg-primary"
            style={{ top: "auto" }}
          />
        </div>
      ))}
    </div>
  );
}

function ProfileNode({ data }: NodeProps<Node<ProfileData>>) {
  return (
    <div className="w-64 rounded-lg border-2 border-primary/40 bg-card p-3 shadow-sm">
      <div className="flex items-center gap-2">
        <GitBranch className="size-4 text-primary" />
        <div className="font-medium">{data.name || "new-profile"}</div>
      </div>
      <div className="mt-1 text-xs text-muted-foreground">
        runtime profile — connect adapter ports
      </div>
      {SLOTS.map((s) => (
        <div
          key={s.key}
          className="relative mt-1.5 rounded bg-primary/10 px-2 py-1 font-mono text-[11px]"
        >
          {s.label}
          <Handle
            type="target"
            position={Position.Left}
            id={s.key}
            className="!size-2.5 !bg-primary"
            style={{ top: "auto" }}
          />
        </div>
      ))}
      <div className="mt-2 border-t pt-1.5 text-[10px] text-muted-foreground">
        {BUILTIN_SLOTS.join(" · ")} → builtin names
      </div>
    </div>
  );
}

const nodeTypes = { adapter: AdapterNode, profile: ProfileNode };

let nodeSeq = 0;

export function PipelinesPage() {
  const [adapters, setAdapters] = useState<Adapter[]>([]);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [configDoc, setConfigDoc] = useState("");
  const [profileName, setProfileName] = useState("my-pipeline");
  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const [yamlOut, setYamlOut] = useState("");
  const [yamlOpen, setYamlOpen] = useState(false);
  const [loadProfile, setLoadProfile] = useState("");

  useEffect(() => {
    Promise.all([getAdapters(), getProfiles(), getConfig()])
      .then(([a, p, c]) => {
        setAdapters(a);
        setProfiles(p);
        setConfigDoc(c.document);
      })
      .catch((e) => toast.error(e.message));
  }, []);

  useEffect(() => {
    setNodes((ns) =>
      ns.some((n) => n.type === "profile")
        ? ns.map((n) =>
            n.type === "profile"
              ? { ...n, data: { ...n.data, name: profileName } }
              : n,
          )
        : [
            ...ns,
            {
              id: "profile",
              type: "profile",
              position: { x: 420, y: 80 },
              data: { name: profileName },
            },
          ],
    );
  }, [profileName, setNodes]);

  const addAdapter = (a: Adapter) => {
    const id = `a${++nodeSeq}`;
    setNodes((ns) => [
      ...ns,
      {
        id,
        type: "adapter",
        position: { x: 40 + (nodeSeq % 3) * 60, y: 40 + nodeSeq * 90 },
        data: { adapter: a },
      },
    ]);
  };

  const onConnect = useCallback(
    (conn: Connection) =>
      setEdges((es) => {
        // One binding per profile slot — replace any existing edge into it.
        const next = es.filter((e) => e.targetHandle !== conn.targetHandle);
        return addEdge(conn, next);
      }),
    [setEdges],
  );

  const generatedYaml = useMemo(() => {
    const bindings = edges
      .filter((e) => e.target === "profile")
      .map((e) => {
        const node = nodes.find((n) => n.id === e.source);
        const a = (node?.data as AdapterData | undefined)?.adapter;
        if (!a?.adapter) return null;
        return { slot: e.targetHandle as string, ref: `${a.adapter.id}@1` };
      })
      .filter(Boolean) as { slot: string; ref: string }[];
    const lines = [
      `  ${profileName || "my-pipeline"}:`,
      `    sandbox: local-process-t0`,
      `    workspace: local-workspace`,
      `    artifact_store: local-artifacts`,
    ];
    for (const b of bindings) lines.push(`    ${b.slot}: ${b.ref}`);
    return lines.join("\n");
  }, [edges, nodes, profileName]);

  const stagedDocument = useMemo(() => {
    if (!configDoc) return "";
    // Insert the new profile under `profiles:` (or append a profiles block).
    if (/^profiles:/m.test(configDoc)) {
      return configDoc.replace(/^profiles:\n/m, `profiles:\n${generatedYaml}\n`);
    }
    return `${configDoc.trimEnd()}\n\nprofiles:\n${generatedYaml}\n`;
  }, [configDoc, generatedYaml]);

  const openYaml = () => {
    setYamlOut(stagedDocument);
    setYamlOpen(true);
  };

  const stage = async () => {
    try {
      const p = await proposeConfig(yamlOut);
      toast.success(
        p.valid
          ? `staged ${p.path} — restart agentd with --config to activate`
          : `staged but invalid: ${p.error}`,
      );
      setYamlOpen(false);
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  const load = (name: string) => {
    setLoadProfile(name);
    const p = profiles.find((x) => x.name === name);
    if (!p) return;
    setProfileName(`${name}-copy`);
    const newEdges: Edge[] = [];
    const newNodes: Node[] = [];
    let i = 0;
    for (const [slot, ref] of Object.entries(p.bindings)) {
      const adapterId = ref.split("@")[0];
      const a = adapters.find((x) => x.adapter?.id === adapterId);
      if (!a) continue;
      const nid = `a${++nodeSeq}`;
      newNodes.push({
        id: nid,
        type: "adapter",
        position: { x: 40, y: 40 + i * 140 },
        data: { adapter: a },
      });
      if (SLOTS.some((s) => s.key === slot))
        newEdges.push({
          id: `e-${nid}-${slot}`,
          source: nid,
          sourceHandle: SLOTS.find((s) => s.key === slot)!.adapterPort,
          target: "profile",
          targetHandle: slot,
        });
      i++;
    }
    setNodes((ns) => [
      ...ns.filter((n) => n.type === "profile"),
      ...newNodes,
    ]);
    setEdges(newEdges);
  };

  return (
    <div className="flex h-full">
      <div className="flex w-64 flex-col border-r">
        <div className="border-b p-3">
          <Label className="text-xs">Profile name</Label>
          <Input
            className="mt-1 h-8"
            value={profileName}
            onChange={(e) => setProfileName(e.target.value)}
          />
          <div className="mt-2">
            <Label className="text-xs">Start from existing</Label>
            <Select
              value={loadProfile}
              onValueChange={(v) => v && load(v)}
            >
              <SelectTrigger className="mt-1 h-8">
                <SelectValue placeholder="load profile…" />
              </SelectTrigger>
              <SelectContent>
                {profiles.map((p) => (
                  <SelectItem key={p.name} value={p.name}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
        <div className="flex-1 space-y-1 overflow-y-auto p-2">
          <div className="px-1 pb-1 text-xs font-medium text-muted-foreground">
            Adapters — click to add
          </div>
          {adapters.map((a) => (
            <button
              key={a.adapter?.id}
              onClick={() => addAdapter(a)}
              className="w-full rounded-md border px-2 py-1.5 text-left hover:bg-accent/50"
            >
              <div className="flex items-center gap-1.5 font-mono text-[11px]">
                <Plus className="size-3" />
                {a.adapter?.id.slice(0, 18)}…
              </div>
              <div className="mt-0.5 flex gap-1">
                <Badge variant="secondary" className="text-[10px]">
                  {a.runtime_type}
                </Badge>
                {(a.ports || []).map((p) => (
                  <Badge key={p} variant="outline" className="text-[10px]">
                    {p}
                  </Badge>
                ))}
              </div>
            </button>
          ))}
        </div>
        <div className="space-y-2 border-t p-3">
          <Button className="w-full" size="sm" onClick={openYaml}>
            <Save className="mr-1.5 size-3.5" /> Generate config
          </Button>
        </div>
      </div>

      <div className="min-w-0 flex-1">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          fitView
          proOptions={{ hideAttribution: true }}
        >
          <Background gap={20} />
          <Controls />
        </ReactFlow>
      </div>

      <Dialog open={yamlOpen} onOpenChange={setYamlOpen}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>Stage config document</DialogTitle>
          </DialogHeader>
          <p className="text-xs text-muted-foreground">
            The kernel activates config at boot — staging writes this document
            to the runtime dir; activate it with{" "}
            <code>agentd --config &lt;path&gt;</code> on the next start.
          </p>
          <Textarea
            className="max-h-96 font-mono text-xs"
            value={yamlOut}
            onChange={(e) => setYamlOut(e.target.value)}
            rows={20}
          />
          <DialogFooter>
            <Button variant="secondary" onClick={() => setYamlOpen(false)}>
              Close
            </Button>
            <Button onClick={stage}>
              <Upload className="mr-1.5 size-3.5" /> Stage proposal
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
