import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Check, X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { getApprovals, respondApproval, type Approval } from "@/lib/api";

export function ApprovalsPage() {
  const [approvals, setApprovals] = useState<Approval[]>([]);

  const load = () =>
    getApprovals()
      .then(setApprovals)
      .catch((e) => toast.error(e.message));

  useEffect(() => {
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, []);

  const respond = async (a: Approval, decision: "approve" | "deny") => {
    try {
      await respondApproval(a.request_id, a.request_digest, decision);
      toast.success(`${decision} recorded`);
      load();
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  return (
    <div className="h-full space-y-4 overflow-auto p-6">
      <h1 className="text-lg font-semibold">Approvals</h1>
      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Pending requests</CardTitle>
          <CardDescription>
            Runs paused on a human decision show up here in real time.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {approvals.map((a) => (
            <div
              key={a.request_id}
              className="space-y-2 rounded-lg border p-3"
            >
              <div className="flex items-center justify-between">
                <span className="font-mono text-xs">{a.request_id}</span>
                <div className="flex items-center gap-1.5">
                  <Badge variant="secondary">{a.operation || "approval"}</Badge>
                  <Badge
                    variant={a.state === "pending" ? "default" : "outline"}
                  >
                    {a.state || "unknown"}
                  </Badge>
                </div>
              </div>
              <div className="space-y-0.5 text-xs text-muted-foreground">
                {a.target_resource && <div>target: {a.target_resource}</div>}
                {a.run_id && (
                  <div className="font-mono">run: {a.run_id}</div>
                )}
                {a.expires_at_ms ? (
                  <div>
                    expires: {new Date(a.expires_at_ms).toLocaleString()}
                  </div>
                ) : null}
              </div>
              {a.state === "pending" && (
                <div className="flex gap-2">
                  <Button size="sm" onClick={() => respond(a, "approve")}>
                    <Check className="mr-1 size-3.5" /> Approve
                  </Button>
                  <Button
                    size="sm"
                    variant="destructive"
                    onClick={() => respond(a, "deny")}
                  >
                    <X className="mr-1 size-3.5" /> Deny
                  </Button>
                </div>
              )}
            </div>
          ))}
          {!approvals.length && (
            <p className="text-sm text-muted-foreground">
              nothing waiting on you
            </p>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
