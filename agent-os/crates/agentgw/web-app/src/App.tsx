import { useEffect, useState } from "react";
import {
  Bot,
  Boxes,
  CheckSquare,
  GitBranch,
  MessageSquare,
  Play,
  Settings,
  SunMoon,
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
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { getHealth, type Health } from "@/lib/api";
import { THEMES, applyTheme, themeById, useTheme } from "@/lib/themes";
import { cn } from "@/lib/utils";
import { AdaptersPage } from "@/pages/AdaptersPage";
import { ApprovalsPage } from "@/pages/ApprovalsPage";
import { ChatPage } from "@/pages/ChatPage";
import { ConfigPage } from "@/pages/ConfigPage";
import { PipelinesPage } from "@/pages/PipelinesPage";
import { RunsPage } from "@/pages/RunsPage";
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
  { id: "pipelines", label: "Workflows", icon: Workflow },
  { id: "adapters", label: "Adapters", icon: Boxes },
  { id: "config", label: "Configuration", icon: GitBranch },
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
    const timer = setInterval(tick, 5000);
    return () => clearInterval(timer);
  }, []);

  const openRun = (runId: string) => {
    setChatRunId(runId);
    setPage("runs");
  };

  return (
    <TooltipProvider>
      <div className="flex h-screen overflow-hidden bg-background text-foreground">
        <aside className="flex w-13 shrink-0 flex-col items-center border-r bg-sidebar py-2">
          <div className="mb-3 flex size-8 items-center justify-center rounded-lg bg-foreground text-background">
            <Bot className="size-4" />
          </div>
          <nav className="flex flex-1 flex-col gap-1">
            {NAV.map(({ id, label, icon: Icon }) => (
              <Tooltip key={id}>
                <TooltipTrigger
                  render={
                    <button
                      onClick={() => setPage(id)}
                      className={cn(
                        "flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
                        page === id &&
                          "bg-sidebar-accent text-sidebar-accent-foreground",
                      )}
                      aria-label={label}
                    >
                      <Icon className="size-4" />
                    </button>
                  }
                />
                <TooltipContent side="right">{label}</TooltipContent>
              </Tooltip>
            ))}
          </nav>
          <Tooltip>
            <TooltipTrigger
              render={
                <div className="relative mb-1">
                  <Select
                    value={theme.id}
                    onValueChange={(value) => value && applyTheme(value)}
                  >
                    <SelectTrigger
                      className="size-8 justify-center rounded-md border-0 bg-transparent p-0 text-muted-foreground shadow-none hover:bg-sidebar-accent hover:text-foreground [&>svg:last-child]:hidden"
                      aria-label="Theme"
                    >
                      <SelectValue>
                        {() => <SunMoon className="size-4" />}
                      </SelectValue>
                    </SelectTrigger>
                    <SelectContent side="right" align="end">
                      {THEMES.map((item) => (
                        <SelectItem key={item.id} value={item.id}>
                          {item.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              }
            />
            <TooltipContent side="right">
              {themeById(theme.id).label}
            </TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger
              render={
                <div className="flex size-8 items-center justify-center">
                  <span
                    className={cn(
                      "size-2 rounded-full ring-4 ring-sidebar",
                      health ? "bg-emerald-500" : "bg-destructive",
                    )}
                  />
                </div>
              }
            />
            <TooltipContent side="right">
              {health ? `Daemon ${health.status}` : "Daemon unreachable"}
            </TooltipContent>
          </Tooltip>
        </aside>
        <main className="min-w-0 flex-1 overflow-hidden">
          {page === "chat" && (
            <ChatPage
              onOpenRun={openRun}
              onOpenWorkflows={() => setPage("pipelines")}
            />
          )}
          {page === "runs" && (
            <RunsPage
              focusRunId={chatRunId}
              onConsumed={() => setChatRunId(null)}
            />
          )}
          {page === "pipelines" && <PipelinesPage />}
          {page === "adapters" && <AdaptersPage />}
          {page === "config" && <ConfigPage />}
          {page === "approvals" && <ApprovalsPage />}
          {page === "system" && <SystemPage />}
        </main>
        <Toaster richColors position="bottom-right" theme={theme.mode} />
      </div>
    </TooltipProvider>
  );
}
