import { useEffect, useState } from "react";
import { toast } from "sonner";
import { GitPullRequest, Upload } from "lucide-react";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";
import {
  getConfig,
  getGenerations,
  getProfiles,
  proposeConfig,
  type Generation,
  type Profile,
} from "@/lib/api";

export function ConfigPage() {
  const [config, setConfig] = useState("");
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [generations, setGenerations] = useState<Generation[]>([]);
  const [editorOpen, setEditorOpen] = useState(false);
  const [draft, setDraft] = useState("");

  const load = () => {
    Promise.all([getConfig(), getProfiles(), getGenerations()])
      .then(([c, p, g]) => {
        setConfig(c.document);
        setProfiles(p);
        setGenerations(g);
      })
      .catch((e) => toast.error(e.message));
  };

  useEffect(() => {
    load();
    const t = setInterval(load, 8000);
    return () => clearInterval(t);
  }, []);

  const stage = async () => {
    try {
      const p = await proposeConfig(draft);
      toast.success(
        p.valid
          ? `staged ${p.path} — activate at next boot with --config`
          : `staged but validation failed: ${p.error}`,
      );
      setEditorOpen(false);
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  return (
    <div className="space-y-4 overflow-auto p-6">
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">Config</h1>
        <Button
          size="sm"
          variant="secondary"
          onClick={() => {
            setDraft(config);
            setEditorOpen(true);
          }}
        >
          <GitPullRequest className="mr-1.5 size-3.5" /> Edit / propose
        </Button>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Profiles</CardTitle>
          <CardDescription>
            Runtime profiles a run can request — pick these in the agent and
            pipeline flows.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Profile</TableHead>
                <TableHead>Bindings</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {profiles.map((p) => (
                <TableRow key={p.name}>
                  <TableCell className="font-medium">{p.name}</TableCell>
                  <TableCell>
                    <div className="flex flex-wrap gap-1">
                      {Object.entries(p.bindings).map(([k, v]) => (
                        <Badge
                          key={k}
                          variant="outline"
                          className="font-mono text-[10px]"
                        >
                          {k}→{v}
                        </Badge>
                      ))}
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Generations</CardTitle>
          <CardDescription>
            Immutable config generations recorded by the daemon.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Generation</TableHead>
                <TableHead>Digest</TableHead>
                <TableHead>Validation</TableHead>
                <TableHead>Tests</TableHead>
                <TableHead>Status</TableHead>
                <TableHead>Created</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {generations.map((g) => (
                <TableRow key={g.generation_id}>
                  <TableCell className="font-mono text-xs">
                    {g.generation_id.slice(0, 18)}…
                  </TableCell>
                  <TableCell className="font-mono text-xs">
                    {g.digest.slice(0, 16)}…
                  </TableCell>
                  <TableCell>{g.validation_state}</TableCell>
                  <TableCell>{g.test_state}</TableCell>
                  <TableCell>
                    <Badge variant={g.active ? "default" : "secondary"}>
                      {g.active ? "active" : "inactive"}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-xs text-muted-foreground">
                    {g.created_at_ms
                      ? new Date(g.created_at_ms).toLocaleString()
                      : "—"}
                  </TableCell>
                </TableRow>
              ))}
              {!generations.length && (
                <TableRow>
                  <TableCell colSpan={6} className="text-muted-foreground">
                    none recorded
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Active document</CardTitle>
        </CardHeader>
        <CardContent>
          <pre className="max-h-80 overflow-auto rounded-md bg-muted p-3 font-mono text-xs whitespace-pre-wrap">
            {config}
          </pre>
        </CardContent>
      </Card>

      <Dialog open={editorOpen} onOpenChange={setEditorOpen}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>Propose config document</DialogTitle>
          </DialogHeader>
          <p className="text-xs text-muted-foreground">
            Validated and written to the runtime dir; the daemon picks it up on
            the next <code>--config</code> boot.
          </p>
          <Textarea
            className="max-h-96 font-mono text-xs"
            rows={20}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <DialogFooter>
            <Button variant="secondary" onClick={() => setEditorOpen(false)}>
              Cancel
            </Button>
            <Button onClick={stage}>
              <Upload className="mr-1.5 size-3.5" /> Validate & stage
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
