import { useEffect, useState } from "react";
import {
  Bot,
  Boxes,
  CheckSquare,
  GitBranch,
  MessageSquare,
  Play,
  Settings,
  Workflow,
} from "lucide-react";
import { Toaster } from "@/components/ui/sonner";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { getHealth, type Health } from "@/lib/api";
import { THEMES, applyTheme, themeById, useTheme } from "@/lib/themes";
import { cn } from "@/lib/utils";
import { ChatPage } from "@/pages/ChatPage";
import { RunsPage } from "@/pages/RunsPage";
import { PipelinesPage } from "@/pages/PipelinesPage";
import { AdaptersPage } from "@/pages/AdaptersPage";
import { ConfigPage } from "@/pages/ConfigPage";
import { ApprovalsPage } from "@/pages/ApprovalsPage";
import { SystemPage } from "@/pages/SystemPage";

type Page =
  | "chat"
  | "runs"
  | "pipelines"
  | "adapters"
  | "config"
  | "approvals"
  | "system";

const NAV: { id: Page; label: string; icon: typeof Bot }[] = [
  { id: "chat", label: "Chat", icon: MessageSquare },
  { id: "runs", label: "Runs", icon: Play },
  { id: "pipelines", label: "Pipelines", icon: Workflow },
  { id: "adapters", label: "Adapters", icon: Boxes },
  { id: "config", label: "Config", icon: GitBranch },
  { id: "approvals", label: "Approvals", icon: CheckSquare },
  { id: "system", label: "System", icon: Settings },
];

export default function App() {
  const [page, setPage] = useState<Page>("chat");
  const [health, setHealth] = useState<Health | null>(null);
  const [chatRunId, setChatRunId] = useState<string | null>(null);
  const theme = useTheme();

  useEffect(() => {
    const tick = () => getHealth().then(setHealth).catch(() => setHealth(null));
    tick();
    const t = setInterval(tick, 5000);
    return () => clearInterval(t);
  }, []);

  const openRun = (runId: string) => {
    setChatRunId(runId);
    setPage("runs");
  };

  return (
    <div className="flex h-screen bg-background text-foreground">
      <aside className="flex w-56 flex-col border-r bg-muted/30">
        <div className="flex items-center gap-2 border-b px-4 py-4">
          <Bot className="size-5" />
          <div className="font-semibold">Agent OS</div>
        </div>
        <nav className="flex-1 space-y-0.5 p-2">
          {NAV.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => setPage(id)}
              className={cn(
                "flex w-full items-center gap-2 rounded-md px-3 py-2 text-sm",
                page === id
                  ? "bg-accent text-accent-foreground font-medium"
                  : "text-muted-foreground hover:bg-accent/50",
              )}
            >
              <Icon className="size-4" /> {label}
            </button>
          ))}
        </nav>
        <div className="border-t p-3 text-xs text-muted-foreground">
          <Select
            value={theme.id}
            onValueChange={(v) => v && applyTheme(v)}
          >
            <SelectTrigger className="mb-3 h-7 w-full text-xs">
              <SelectValue>
                {(v: unknown) => themeById(String(v)).label}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {THEMES.map((t) => (
                <SelectItem key={t.id} value={t.id}>
                  {t.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <div className="flex items-center gap-1.5">
            <span
              className={cn(
                "size-2 rounded-full",
                health ? "bg-emerald-500" : "bg-red-500",
              )}
            />
            {health ? `daemon ${health.status}` : "daemon unreachable"}
          </div>
          {health && (
            <div className="mt-1 font-mono opacity-70">
              epoch {health.daemon_fencing_epoch} · outbox{" "}
              {health.outbox_unpublished_count}
            </div>
          )}
        </div>
      </aside>
      <main className="min-w-0 flex-1 overflow-hidden">
        {page === "chat" && <ChatPage onOpenRun={openRun} />}
        {page === "runs" && (
          <RunsPage focusRunId={chatRunId} onConsumed={() => setChatRunId(null)} />
        )}
        {page === "pipelines" && <PipelinesPage />}
        {page === "adapters" && <AdaptersPage />}
        {page === "config" && <ConfigPage />}
        {page === "approvals" && <ApprovalsPage />}
        {page === "system" && <SystemPage />}
      </main>
      <Toaster richColors position="bottom-right" theme={theme.mode} />
    </div>
  );
}
