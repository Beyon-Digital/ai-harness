import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MiniMap,
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
import {
  Boxes,
  GitBranch,
  GripVertical,
  Plus,
  Save,
  Trash2,
  Upload,
} from "lucide-react";
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
import { useTheme } from "@/lib/themes";

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

function AdapterNode({ data, selected }: NodeProps<Node<AdapterData>>) {
  const a = data.adapter;
  return (
    <div
      className={`w-64 overflow-hidden rounded-lg border bg-card shadow-sm transition-shadow ${
        selected ? "ring-2 ring-primary" : "hover:shadow-md"
      }`}
    >
      <div className="flex items-center gap-2 border-b bg-muted/60 px-3 py-2">
        <GripVertical className="size-3.5 shrink-0 text-muted-foreground/60" />
        <Boxes className="size-4 shrink-0 text-primary" />
        <div className="truncate font-mono text-xs font-medium">
          {a.adapter?.id ?? "adapter"}
        </div>
        <Badge variant="outline" className="ml-auto shrink-0">
          {a.runtime_type}
        </Badge>
      </div>
      <div className="px-3 pt-1.5 text-xs text-muted-foreground">
        v{a.adapter?.version} · {a.trust_state}
      </div>
      <div className="space-y-1 px-3 py-2">
        {(a.ports || []).map((p) => (
          <div
            key={p}
            className="relative rounded border border-dashed border-primary/40 bg-primary/5 px-2 py-1 font-mono text-[11px]"
          >
            {p}
            <Handle
              type="source"
              position={Position.Right}
              id={p}
              className="!size-2.5 !border-2 !border-card !bg-primary"
            />
          </div>
        ))}
        {!a.ports?.length && (
          <div className="text-[11px] text-muted-foreground">no ports</div>
        )}
      </div>
    </div>
  );
}

function ProfileNode({ data, selected }: NodeProps<Node<ProfileData>>) {
  return (
    <div
      className={`w-64 overflow-hidden rounded-lg border-2 bg-card shadow-sm transition-shadow ${
        selected
          ? "border-primary ring-2 ring-primary/40"
          : "border-primary/50 hover:shadow-md"
      }`}
    >
      <div className="flex items-center gap-2 border-b bg-primary/10 px-3 py-2">
        <GitBranch className="size-4 shrink-0 text-primary" />
        <div className="truncate text-sm font-semibold">
          {data.name || "new-profile"}
        </div>
      </div>
      <div className="px-3 pt-1.5 text-xs text-muted-foreground">
        runtime profile — wire adapter ports into the slots below
      </div>
      <div className="space-y-1 px-3 py-2">
        {SLOTS.map((s) => (
          <div
            key={s.key}
            className="relative rounded border border-primary/30 bg-primary/10 px-2 py-1 font-mono text-[11px]"
          >
            <Handle
              type="target"
              position={Position.Left}
              id={s.key}
              className="!size-2.5 !border-2 !border-card !bg-primary"
            />
            {s.label}
          </div>
        ))}
      </div>
      <div className="border-t px-3 py-1.5 text-[10px] text-muted-foreground">
        {BUILTIN_SLOTS.join(" · ")} → builtin names
      </div>
    </div>
  );
}

const nodeTypes = { adapter: AdapterNode, profile: ProfileNode };

let nodeSeq = 0;

export function PipelinesPage() {
  const theme = useTheme();
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
              position: { x: 460, y: 120 },
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
        position: { x: 40 + (nodeSeq % 2) * 40, y: 60 + nodeSeq * 100 },
        data: { adapter: a },
      },
    ]);
  };

  const onConnect = useCallback(
    (conn: Connection) =>
      setEdges((es) => {
        // One binding per profile slot — replace any existing edge into it.
        const next = es.filter((e) => e.targetHandle !== conn.targetHandle);
        return addEdge(
          {
            ...conn,
            type: "smoothstep",
            label: conn.targetHandle ?? undefined,
          },
          next,
        );
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
      // Bindings are `adapter@version` strings; coerce anything else for safety.
      const refStr = typeof ref === "string" ? ref : JSON.stringify(ref);
      const adapterId = refStr.split("@")[0];
      const a = adapters.find((x) => x.adapter?.id === adapterId);
      if (!a) continue;
      const nid = `a${++nodeSeq}`;
      newNodes.push({
        id: nid,
        type: "adapter",
        position: { x: 40, y: 60 + i * 150 },
        data: { adapter: a },
      });
      if (SLOTS.some((s) => s.key === slot))
        newEdges.push({
          id: `e-${nid}-${slot}`,
          type: "smoothstep",
          label: slot,
          source: nid,
          sourceHandle: SLOTS.find((s) => s.key === slot)!.adapterPort,
          target: "profile",
          targetHandle: slot,
        });
      i++;
    }
    setNodes((ns) => [...ns.filter((n) => n.type === "profile"), ...newNodes]);
    setEdges(newEdges);
  };

  const clearCanvas = () => {
    setNodes((ns) => ns.filter((n) => n.type === "profile"));
    setEdges([]);
  };

  const edgeCount = edges.length;

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
          <div className="mt-3">
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
        <div className="flex-1 space-y-1.5 overflow-y-auto p-2">
          <div className="px-1 pb-1 text-xs font-medium text-muted-foreground">
            Adapters — click to add
          </div>
          {adapters.map((a) => (
            <button
              key={a.adapter?.id}
              onClick={() => addAdapter(a)}
              className="w-full rounded-md border bg-card px-2 py-1.5 text-left transition-colors hover:border-primary/50 hover:bg-accent/50"
            >
              <div className="flex items-center gap-1.5 font-mono text-[11px]">
                <Plus className="size-3 shrink-0" />
                <span className="truncate">{a.adapter?.id}</span>
              </div>
              <div className="mt-1 flex flex-wrap gap-1">
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
          <div className="text-xs text-muted-foreground">
            {edgeCount} slot{edgeCount === 1 ? "" : "s"} bound — drag from an
            adapter port to a profile slot. Delete removes the selected node
            or edge.
          </div>
          <div className="flex gap-2">
            <Button className="flex-1" size="sm" onClick={openYaml}>
              <Save className="mr-1.5 size-3.5" /> Generate config
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={clearCanvas}
              title="Clear adapters from canvas"
            >
              <Trash2 className="size-3.5" />
            </Button>
          </div>
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
          colorMode={theme.mode}
          fitView
          snapToGrid
          snapGrid={[16, 16]}
          deleteKeyCode={["Backspace", "Delete"]}
          defaultEdgeOptions={{ type: "smoothstep" }}
          proOptions={{ hideAttribution: true }}
        >
          <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
          <MiniMap
            pannable
            zoomable
            className="!bg-card"
            nodeColor="var(--primary)"
            maskColor="color-mix(in srgb, var(--background) 70%, transparent)"
          />
          <Controls showInteractive={false} />
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
