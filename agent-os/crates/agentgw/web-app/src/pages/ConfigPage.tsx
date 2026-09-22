import { useEffect, useState } from "react";
import { toast } from "sonner";
import { FileCode2, GitPullRequest, Upload } from "lucide-react";
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
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import {
  getConfig,
  getGenerations,
  getProfiles,
  getProposals,
  proposeConfig,
  type ConfigProposal,
  type Generation,
  type Profile,
} from "@/lib/api";

export function ConfigPage() {
  const [config, setConfig] = useState("");
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [generations, setGenerations] = useState<Generation[]>([]);
  const [proposals, setProposals] = useState<ConfigProposal[]>([]);
  const [editorOpen, setEditorOpen] = useState(false);
  const [draft, setDraft] = useState("");

  const load = () => {
    Promise.all([getConfig(), getProfiles(), getGenerations(), getProposals()])
      .then(([c, p, g, pr]) => {
        setConfig(c.document);
        setProfiles(p);
        setGenerations(g);
        setProposals(pr);
      })
      .catch((e) => toast.error(e.message));
  };

  useEffect(() => {
    load();
    const t = setInterval(load, 8000);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
      load();
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  return (
    <div className="space-y-5 overflow-auto p-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-lg font-semibold">Config</h1>
          <p className="mt-0.5 text-sm text-muted-foreground">
            Runtime profiles and immutable config generations. Proposals are
            validated and staged for the next daemon boot.
          </p>
        </div>
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

      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="advanced">
            <FileCode2 className="size-3.5" /> Advanced (raw files)
          </TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="mt-4 space-y-5">
          <Card>
            <CardHeader>
              <CardTitle className="text-sm">Profiles</CardTitle>
              <CardDescription>
                A profile bundles which adapters a run uses — pick these when
                creating agents or designing pipelines.
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
                  {!profiles.length && (
                    <TableRow>
                      <TableCell
                        colSpan={2}
                        className="text-muted-foreground"
                      >
                        no profiles in the active document
                      </TableCell>
                    </TableRow>
                  )}
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
        </TabsContent>

        <TabsContent value="advanced" className="mt-4 space-y-5">
          <Card>
            <CardHeader>
              <CardTitle className="text-sm">Staged proposals</CardTitle>
              <CardDescription>
                Config documents staged under the runtime dir — each is
                picked up by the next <code>agentd --config</code> boot.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Proposal</TableHead>
                    <TableHead>Valid</TableHead>
                    <TableHead>Created</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {proposals.map((p) => (
                    <TableRow key={p.proposal_id}>
                      <TableCell className="font-mono text-xs">
                        {p.proposal_id.slice(0, 18)}…
                      </TableCell>
                      <TableCell>
                        <Badge
                          variant={p.valid ? "default" : "destructive"}
                          title={p.error}
                        >
                          {p.valid ? "valid" : "invalid"}
                        </Badge>
                        {!p.valid && p.error && (
                          <div className="mt-1 max-w-72 truncate text-[11px] text-muted-foreground">
                            {p.error}
                          </div>
                        )}
                      </TableCell>
                      <TableCell className="text-xs text-muted-foreground">
                        {p.created_at_ms
                          ? new Date(p.created_at_ms).toLocaleString()
                          : "—"}
                      </TableCell>
                    </TableRow>
                  ))}
                  {!proposals.length && (
                    <TableRow>
                      <TableCell colSpan={3} className="text-muted-foreground">
                        nothing staged — use Edit / propose
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
              <CardDescription>
                The raw YAML the kernel is running — inspect or copy into the
                editor via Edit / propose.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <pre className="max-h-96 overflow-auto rounded-md bg-muted p-3 font-mono text-xs whitespace-pre-wrap">
                {config}
              </pre>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>

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
