import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Plug, Plus, Trash2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
  TOKEN_KEY,
  flattenMetrics,
  getHealth,
  getMetrics,
  type Health,
} from "@/lib/api";
import {
  PROVIDERS_KEY,
  loadProviders,
  type Provider,
} from "@/lib/providers";

export function SystemPage() {
  const [health, setHealth] = useState<Health | null>(null);
  const [metrics, setMetrics] = useState<{ name: string; value: string }[]>(
    [],
  );
  const [providers, setProviders] = useState<Provider[]>(loadProviders);
  const [addOpen, setAddOpen] = useState(false);
  const [np, setNp] = useState({ name: "", baseUrl: "", model: "", keyEnv: "" });
  const [gwToken, setGwToken] = useState(
    () => localStorage.getItem(TOKEN_KEY) ?? "",
  );

  useEffect(() => {
    const load = () => {
      getHealth().then(setHealth).catch(() => setHealth(null));
      getMetrics()
        .then((m) => setMetrics(flattenMetrics(m)))
        .catch(() => setMetrics([]));
    };
    load();
    const t = setInterval(load, 5000);
    return () => clearInterval(t);
  }, []);

  const save = (next: Provider[]) => {
    setProviders(next);
    localStorage.setItem(PROVIDERS_KEY, JSON.stringify(next));
  };

  const addProvider = () => {
    if (!np.name || !np.baseUrl) {
      toast.error("name and base URL are required");
      return;
    }
    save([...providers, { ...np, id: crypto.randomUUID() }]);
    setNp({ name: "", baseUrl: "", model: "", keyEnv: "" });
    setAddOpen(false);
    toast.success(`provider "${np.name}" added — pick it in Chat`);
  };

  return (
    <div className="space-y-4 overflow-auto p-6">
      <h1 className="text-lg font-semibold">System</h1>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Inference providers</CardTitle>
          <CardDescription>
            OpenAI-compatible chat-completions endpoints. Runs carry{" "}
            <code>base_url</code>/<code>model</code>/<code>api_key_env</code>{" "}
            to the effect adapter — credentials live in the daemon&apos;s
            environment, never in the browser or run payloads.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-2">
          {providers.map((p) => (
            <div
              key={p.id}
              className="flex items-center gap-3 rounded-lg border p-3"
            >
              <Plug className="size-4 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium">{p.name}</div>
                <div className="truncate font-mono text-xs text-muted-foreground">
                  {p.baseUrl} · {p.model || "default model"}
                  {p.keyEnv ? ` · key env ${p.keyEnv}` : ""}
                </div>
              </div>
              {p.builtin ? (
                <Badge variant="secondary">built-in</Badge>
              ) : (
                <Button
                  size="icon"
                  variant="ghost"
                  onClick={() =>
                    save(providers.filter((x) => x.id !== p.id))
                  }
                >
                  <Trash2 className="size-4" />
                </Button>
              )}
            </div>
          ))}
          <Button
            variant="outline"
            size="sm"
            onClick={() => setAddOpen(true)}
          >
            <Plus className="mr-1.5 size-3.5" /> Add OpenAI-compatible provider
          </Button>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Gateway access token</CardTitle>
          <CardDescription>
            Bearer token for a gateway bound with{" "}
            <code>--auth-token</code>/<code>AGENTGW_TOKEN</code> — needed
            when pointing this UI (or the desktop app) at a remote{" "}
            <code>agentgw</code>. You can also open{" "}
            <code>?token=…</code> once to store it.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex items-center gap-2">
          <Input
            type="password"
            className="max-w-sm font-mono"
            placeholder="leave empty for loopback/local"
            value={gwToken}
            onChange={(e) => setGwToken(e.target.value)}
          />
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              if (gwToken.trim()) {
                localStorage.setItem(TOKEN_KEY, gwToken.trim());
              } else {
                localStorage.removeItem(TOKEN_KEY);
              }
              toast.success("gateway token saved — reloading");
              setTimeout(() => window.location.reload(), 400);
            }}
          >
            Save
          </Button>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Daemon</CardTitle>
        </CardHeader>
        <CardContent className="space-y-1 text-xs">
          {health ? (
            Object.entries(health).map(([k, v]) => (
              <div key={k} className="flex justify-between gap-4">
                <span className="text-muted-foreground">{k}</span>
                <span className="truncate font-mono text-right">
                  {typeof v === "object" ? JSON.stringify(v) : String(v)}
                </span>
              </div>
            ))
          ) : (
            <p className="text-muted-foreground">daemon unreachable</p>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Metrics</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-2 gap-2 md:grid-cols-3">
            {metrics.map((m) => (
              <div key={m.name} className="rounded-lg border p-3">
                <div className="text-[11px] text-muted-foreground">
                  {m.name}
                </div>
                <div className="font-mono text-lg">{m.value}</div>
              </div>
            ))}
            {!metrics.length && (
              <p className="col-span-full text-sm text-muted-foreground">
                no metrics yet
              </p>
            )}
          </div>
        </CardContent>
      </Card>

      <Dialog open={addOpen} onOpenChange={setAddOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add provider</DialogTitle>
          </DialogHeader>
          <div className="space-y-3">
            <div>
              <Label>Name</Label>
              <Input
                value={np.name}
                onChange={(e) => setNp({ ...np, name: e.target.value })}
                placeholder="groq / together / local vllm…"
              />
            </div>
            <div>
              <Label>Base URL (chat completions endpoint root)</Label>
              <Input
                value={np.baseUrl}
                onChange={(e) => setNp({ ...np, baseUrl: e.target.value })}
                placeholder="https://api.example.com/v1"
              />
            </div>
            <div>
              <Label>Default model</Label>
              <Input
                value={np.model}
                onChange={(e) => setNp({ ...np, model: e.target.value })}
                placeholder="llama-3.3-70b-versatile"
              />
            </div>
            <div>
              <Label>API key env var (on the daemon host)</Label>
              <Input
                value={np.keyEnv}
                onChange={(e) => setNp({ ...np, keyEnv: e.target.value })}
                placeholder="PROVIDER_KEY_TOGETHER"
              />
              <p className="mt-1 text-[11px] text-muted-foreground">
                The key itself stays in the daemon&apos;s environment —
                only the variable name is sent with runs.
              </p>
            </div>
          </div>
          <DialogFooter>
            <Button variant="secondary" onClick={() => setAddOpen(false)}>
              Cancel
            </Button>
            <Button onClick={addProvider}>Add provider</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
