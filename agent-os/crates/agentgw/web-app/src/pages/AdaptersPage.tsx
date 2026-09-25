import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { getAdapters, type Adapter } from "@/lib/api";

export function AdaptersPage() {
  const [adapters, setAdapters] = useState<Adapter[]>([]);

  useEffect(() => {
    const load = () =>
      getAdapters()
        .then(setAdapters)
        .catch((e) => toast.error(e.message));
    load();
    const t = setInterval(load, 5000);
    return () => clearInterval(t);
  }, []);

  return (
    <div className="h-full space-y-4 overflow-auto p-6">
      <h1 className="text-lg font-semibold">Adapters</h1>
      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Registry</CardTitle>
          <CardDescription>
            Content-addressed adapter bundles registered with the daemon.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Adapter</TableHead>
                <TableHead>Version</TableHead>
                <TableHead>Runtime</TableHead>
                <TableHead>Trust</TableHead>
                <TableHead>Conformance</TableHead>
                <TableHead>Ports</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {adapters.map((a) => (
                <TableRow key={a.adapter?.id}>
                  <TableCell className="font-mono text-xs">
                    {a.adapter?.id}
                  </TableCell>
                  <TableCell>{a.adapter?.version}</TableCell>
                  <TableCell>
                    <Badge variant="secondary">{a.runtime_type}</Badge>
                  </TableCell>
                  <TableCell>{a.trust_state}</TableCell>
                  <TableCell>
                    <Badge
                      variant={
                        a.conformance_state === "passed"
                          ? "default"
                          : a.conformance_state?.includes("fail")
                            ? "destructive"
                            : "outline"
                      }
                    >
                      {a.conformance_state || "—"}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    <div className="flex flex-wrap gap-1">
                      {(a.ports || []).map((p) => (
                        <Badge
                          key={p}
                          variant="outline"
                          className="font-mono text-[10px]"
                        >
                          {p}
                        </Badge>
                      ))}
                    </div>
                  </TableCell>
                </TableRow>
              ))}
              {!adapters.length && (
                <TableRow>
                  <TableCell colSpan={6} className="text-muted-foreground">
                    no adapters registered
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
