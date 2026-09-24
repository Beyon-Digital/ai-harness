import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowUp,
  Bot,
  ChevronDown,
  ExternalLink,
  Monitor,
  Plus,
  Square,
  Workflow,
} from "lucide-react";
import { toast } from "sonner";
import {
  Conversation,
  ConversationContent,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { ToolActivity } from "@/components/ToolActivity";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
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
  cancelRun,
  createRun,
  createSession,
  decodeDataUri,
  getDecisions,
  getIndex,
  getProfiles,
  getRun,
  putSpec,
  uuidv7,
  type Decision,
  type Profile,
  type Spec,
} from "@/lib/api";
import { loadProviders, type Provider } from "@/lib/providers";
import { cn } from "@/lib/utils";

interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  text: string;
  runId?: string;
  state?: string;
  revision?: number;
  decisions?: Decision[];
  error?: boolean;
}

const STARTERS = [
  "Inspect the current screen and summarize what is open",
  "Use the computer to complete the task in the active app",
  "Plan and run a custom agent workflow",
];

function describeDecision(decision: Decision): string | undefined {
  const detail = decision.detail ?? {};
  const value =
    detail.reason_code ?? detail.reason ?? detail.operation ?? detail.output_ref;
  if (value == null) return undefined;
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return text.length > 120 ? `${text.slice(0, 120)}…` : text;
}

function statusLabel(state?: string) {
  if (!state || state === "created") return "Starting";
  if (state === "completed") return "Done";
  if (state === "failed") return "Failed";
  if (state === "cancelled") return "Cancelled";
  return state.replaceAll("_", " ");
}

export function ChatPage({
  onOpenRun,
  onOpenWorkflows,
}: {
  onOpenRun: (runId: string) => void;
  onOpenWorkflows: () => void;
}) {
  const [specs, setSpecs] = useState<Spec[]>([]);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [specId, setSpecId] = useState("");
  const [sessionId, setSessionId] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [providers] = useState<Provider[]>(loadProviders);
  const [providerId, setProviderId] = useState("openrouter");
  const [toolsOn, setToolsOn] = useState(true);
  const [newAgentOpen, setNewAgentOpen] = useState(false);
  const [agentName, setAgentName] = useState("");
  const [agentProfile, setAgentProfile] = useState("");
  const [agentDesc, setAgentDesc] = useState("");
  const [activeRunId, setActiveRunId] = useState("");
  const pollRef = useRef<ReturnType<typeof setInterval>>(undefined);

  const selectedSpec = useMemo(() => {
    const [id, version] = specId.split("@");
    return specs.find(
      (spec) => spec.agent_spec_id === id && spec.version === version,
    );
  }, [specId, specs]);

  const refreshSpecs = useCallback(async () => {
    try {
      const [index, nextProfiles] = await Promise.all([
        getIndex(),
        getProfiles(),
      ]);
      setSpecs(index.specs);
      setProfiles(nextProfiles);
      setSpecId((current) =>
        current || !index.specs.length
          ? current
          : `${index.specs[0].agent_spec_id}@${index.specs[0].version}`,
      );
      setAgentProfile((current) => current || nextProfiles[0]?.name || "");
    } catch (error) {
      toast.error((error as Error).message);
    }
  }, []);

  useEffect(() => {
    refreshSpecs();
    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, [refreshSpecs]);

  const ensureSession = async () => {
    if (sessionId) return sessionId;
    const session = await createSession(uuidv7(), "Agent OS chat");
    setSessionId(session.session_id);
    return session.session_id;
  };

  const pollRun = useCallback((runId: string, messageId: string) => {
    const tick = async () => {
      try {
        const [run, decisions] = await Promise.all([
          getRun(runId),
          getDecisions(runId).catch(() => [] as Decision[]),
        ]);
        const decoded = decodeDataUri(run.output_ref);
        const lastDecision = decisions.at(-1);
        const reason = lastDecision
          ? describeDecision(lastDecision)
          : undefined;
        setMessages((current) =>
          current.map((message) =>
            message.id === messageId
              ? {
                  ...message,
                  state: run.state_name,
                  revision: run.run_revision,
                  decisions,
                  error: run.state === 10 || run.state === 11,
                  text:
                    decoded ||
                    (run.state === 9
                      ? "(no text output)"
                      : run.state === 10
                        ? `Run failed${reason ? `: ${reason}` : ""}`
                        : run.state === 11
                          ? "Run cancelled"
                          : message.text),
                }
              : message,
          ),
        );
        if (run.state >= 9 && pollRef.current) {
          clearInterval(pollRef.current);
          pollRef.current = undefined;
          setBusy(false);
          setActiveRunId("");
        }
      } catch {
        return;
      }
    };
    pollRef.current = setInterval(tick, 1000);
    tick();
  }, []);

  const send = async (draft = input) => {
    const task = draft.trim();
    if (!task || busy) return;
    if (!selectedSpec) {
      toast.error("Create or select an agent first");
      return;
    }
    setBusy(true);
    setInput("");
    const userMessage: ChatMessage = {
      id: uuidv7(),
      role: "user",
      text: task,
    };
    const assistantId = uuidv7();
    setMessages((current) => [
      ...current,
      userMessage,
      {
        id: assistantId,
        role: "assistant",
        text: "",
        state: "created",
      },
    ]);
    try {
      const sid = await ensureSession();
      const provider = providers.find((item) => item.id === providerId);
      const payload = JSON.stringify({
        task,
        ...(provider
          ? {
              model: provider.model,
              base_url: provider.baseUrl,
              ...(provider.keyEnv ? { api_key_env: provider.keyEnv } : {}),
            }
          : {}),
        ...(toolsOn ? { tools: true } : {}),
      });
      const { run_id } = await createRun({
        sessionId: sid,
        specId: selectedSpec.agent_spec_id,
        specVersion: selectedSpec.version,
        specDigest: selectedSpec.digest,
        taskPayload: payload,
        profile: selectedSpec.profile,
      });
      setActiveRunId(run_id);
      setMessages((current) =>
        current.map((message) =>
          message.id === assistantId
            ? { ...message, runId: run_id }
            : message,
        ),
      );
      pollRun(run_id, assistantId);
    } catch (error) {
      setBusy(false);
      setActiveRunId("");
      setMessages((current) =>
        current.map((message) =>
          message.id === assistantId
            ? {
                ...message,
                text: (error as Error).message,
                error: true,
                state: "failed",
              }
            : message,
        ),
      );
    }
  };

  const stop = async () => {
    const message = [...messages]
      .reverse()
      .find((item) => item.runId === activeRunId);
    if (!activeRunId || !message) return;
    try {
      await cancelRun(
        activeRunId,
        "cancelled from chat",
        message.revision ?? 0,
      );
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  const createAgent = async () => {
    if (!agentName.trim()) return;
    try {
      await putSpec(
        uuidv7(),
        "0.1.0",
        JSON.stringify({
          display_name: agentName.trim(),
          runtime_profile_name: agentProfile,
          description: agentDesc.trim(),
        }),
      );
      toast.success(`Created ${agentName.trim()}`);
      setNewAgentOpen(false);
      setAgentName("");
      setAgentDesc("");
      refreshSpecs();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="flex h-full min-w-0 bg-background">
      <aside className="hidden w-56 shrink-0 flex-col border-r bg-sidebar/60 md:flex">
        <div className="flex h-12 items-center justify-between px-3">
          <span className="text-xs font-medium text-muted-foreground">
            Agents
          </span>
          <Dialog open={newAgentOpen} onOpenChange={setNewAgentOpen}>
            <DialogTrigger
              render={
                <Button size="icon" variant="ghost" className="size-7">
                  <Plus className="size-3.5" />
                </Button>
              }
            />
            <DialogContent className="sm:max-w-md">
              <DialogHeader>
                <DialogTitle>Create agent</DialogTitle>
              </DialogHeader>
              <div className="space-y-4">
                <div className="space-y-1.5">
                  <Label>Name</Label>
                  <Input
                    value={agentName}
                    onChange={(event) => setAgentName(event.target.value)}
                    placeholder="Research assistant"
                  />
                </div>
                <div className="space-y-1.5">
                  <Label>Workflow</Label>
                  <Select
                    value={agentProfile}
                    onValueChange={(value) => value && setAgentProfile(value)}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="Select a runtime profile" />
                    </SelectTrigger>
                    <SelectContent>
                      {profiles.map((profile) => (
                        <SelectItem key={profile.name} value={profile.name}>
                          {profile.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1.5">
                  <Label>Description</Label>
                  <Textarea
                    value={agentDesc}
                    onChange={(event) => setAgentDesc(event.target.value)}
                    placeholder="What this agent is best at"
                    className="min-h-20 resize-none"
                  />
                </div>
              </div>
              <DialogFooter>
                <Button onClick={createAgent} disabled={!agentName.trim()}>
                  Create
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </div>
        <div className="flex-1 space-y-0.5 overflow-y-auto px-2">
          {specs.map((spec) => {
            const key = `${spec.agent_spec_id}@${spec.version}`;
            return (
              <button
                key={key}
                onClick={() => setSpecId(key)}
                className={cn(
                  "w-full rounded-md px-2.5 py-2 text-left transition-colors",
                  specId === key
                    ? "bg-sidebar-accent text-sidebar-accent-foreground"
                    : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground",
                )}
              >
                <div className="flex items-center gap-2 text-sm">
                  <Bot className="size-3.5 shrink-0" />
                  <span className="truncate font-medium">
                    {spec.display_name || "Agent"}
                  </span>
                </div>
                <div className="mt-1 truncate pl-5.5 text-[11px] opacity-70">
                  {spec.profile || `v${spec.version}`}
                </div>
              </button>
            );
          })}
          {!specs.length && (
            <button
              className="w-full rounded-md border border-dashed p-4 text-center text-xs text-muted-foreground hover:bg-muted/50"
              onClick={() => setNewAgentOpen(true)}
            >
              Create your first agent
            </button>
          )}
        </div>
        <button
          onClick={onOpenWorkflows}
          className="m-2 flex items-center gap-2 rounded-md px-2.5 py-2 text-xs text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
        >
          <Workflow className="size-3.5" />
          Compose workflows
        </button>
      </aside>

      <section className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-12 shrink-0 items-center gap-2 border-b px-4">
          <Bot className="size-4" />
          <span className="truncate text-sm font-medium">
            {selectedSpec?.display_name || "New conversation"}
          </span>
          {selectedSpec?.profile && (
            <Badge variant="secondary" className="h-5 rounded px-1.5 text-[10px]">
              {selectedSpec.profile}
            </Badge>
          )}
          <div className="ml-auto flex items-center gap-1.5 text-[11px] text-muted-foreground">
            <span
              className={cn(
                "size-1.5 rounded-full",
                toolsOn ? "bg-emerald-500" : "bg-muted-foreground/40",
              )}
            />
            {toolsOn ? "computer enabled" : "tools off"}
          </div>
        </header>

        <Conversation className="flex-1">
          <ConversationContent className="mx-auto w-full max-w-3xl gap-7 px-5 py-8">
            {!messages.length && (
              <div className="flex min-h-[55vh] flex-col justify-center">
                <div className="mb-8">
                  <div className="mb-3 flex size-9 items-center justify-center rounded-lg border bg-card shadow-sm">
                    <Bot className="size-4.5" />
                  </div>
                  <h1 className="text-xl font-semibold tracking-tight">
                    What should we work on?
                  </h1>
                  <p className="mt-1 max-w-lg text-sm text-muted-foreground">
                    Ask an agent to reason, run workflows, and use the computer.
                    Every action stays attached to the conversation.
                  </p>
                </div>
                <div className="grid gap-2 sm:grid-cols-3">
                  {STARTERS.map((starter) => (
                    <button
                      key={starter}
                      onClick={() => setInput(starter)}
                      className="rounded-lg border bg-card/50 p-3 text-left text-xs leading-relaxed text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                    >
                      {starter}
                    </button>
                  ))}
                </div>
              </div>
            )}
            {messages.map((message) => (
              <article
                key={message.id}
                className={cn(
                  "group flex min-w-0 gap-3",
                  message.role === "user" && "justify-end",
                )}
              >
                {message.role === "assistant" && (
                  <div className="mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-md border bg-card">
                    <Bot className="size-3.5" />
                  </div>
                )}
                <div
                  className={cn(
                    "min-w-0 text-sm leading-6",
                    message.role === "user"
                      ? "max-w-[85%] rounded-xl bg-muted px-3.5 py-2"
                      : "max-w-[calc(100%-2.25rem)] flex-1",
                    message.error && "text-destructive",
                  )}
                >
                  {message.role === "assistant" &&
                  !message.text &&
                  message.state !== "failed" ? (
                    <div className="flex items-center gap-2 py-0.5 text-muted-foreground">
                      <span className="flex gap-1">
                        <span className="size-1 animate-pulse rounded-full bg-current" />
                        <span className="size-1 animate-pulse rounded-full bg-current [animation-delay:120ms]" />
                        <span className="size-1 animate-pulse rounded-full bg-current [animation-delay:240ms]" />
                      </span>
                      {statusLabel(message.state)}
                    </div>
                  ) : (
                    <div className="whitespace-pre-wrap">{message.text}</div>
                  )}
                  {message.role === "assistant" && (
                    <>
                      <ToolActivity decisions={message.decisions ?? []} />
                      <div className="mt-2 flex items-center gap-2 text-[11px] text-muted-foreground">
                        {message.state && <span>{statusLabel(message.state)}</span>}
                        {message.runId && (
                          <button
                            className="inline-flex items-center gap-1 hover:text-foreground"
                            onClick={() => onOpenRun(message.runId!)}
                          >
                            Open run
                            <ExternalLink className="size-3" />
                          </button>
                        )}
                      </div>
                    </>
                  )}
                </div>
              </article>
            ))}
          </ConversationContent>
          <ConversationScrollButton className="bottom-3" />
        </Conversation>

        <div className="shrink-0 px-4 pb-4">
          <div className="mx-auto max-w-3xl rounded-xl border bg-card shadow-[0_10px_35px_-18px_rgba(0,0,0,0.35)]">
            <Textarea
              className="min-h-14 max-h-44 resize-none border-0 bg-transparent px-4 py-3 shadow-none focus-visible:ring-0"
              placeholder={
                selectedSpec
                  ? "Ask anything or describe a computer task"
                  : "Create an agent to start"
              }
              value={input}
              disabled={!selectedSpec}
              onChange={(event) => setInput(event.target.value)}
              onKeyDown={(event) => {
                if (
                  event.key === "Enter" &&
                  !event.shiftKey &&
                  !event.nativeEvent.isComposing
                ) {
                  event.preventDefault();
                  send();
                }
              }}
            />
            <div className="flex items-center gap-1.5 px-2.5 pb-2.5">
              <Select
                value={providerId}
                onValueChange={(value) => value && setProviderId(value)}
              >
                <SelectTrigger className="h-7 w-auto max-w-44 gap-1 rounded-md border-0 bg-muted/70 px-2 text-[11px] shadow-none">
                  <SelectValue>
                    {(value: unknown) => {
                      const provider = providers.find(
                        (item) => item.id === value,
                      );
                      return provider
                        ? `${provider.name} · ${provider.model}`
                        : "Default model";
                    }}
                  </SelectValue>
                  <ChevronDown className="size-3" />
                </SelectTrigger>
                <SelectContent>
                  {providers.map((provider) => (
                    <SelectItem key={provider.id} value={provider.id}>
                      {provider.name} · {provider.model}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <button
                onClick={() => setToolsOn((value) => !value)}
                className={cn(
                  "flex h-7 items-center gap-1.5 rounded-md px-2 text-[11px] transition-colors",
                  toolsOn
                    ? "bg-foreground text-background"
                    : "bg-muted/70 text-muted-foreground hover:text-foreground",
                )}
              >
                <Monitor className="size-3" />
                Computer
              </button>
              <div className="flex-1" />
              <span className="hidden text-[10px] text-muted-foreground sm:block">
                Enter to send · Shift Enter for newline
              </span>
              {busy ? (
                <Button
                  size="icon"
                  variant="outline"
                  className="size-8 rounded-lg"
                  onClick={stop}
                >
                  <Square className="size-3 fill-current" />
                  <span className="sr-only">Stop run</span>
                </Button>
              ) : (
                <Button
                  size="icon"
                  className="size-8 rounded-lg"
                  onClick={() => send()}
                  disabled={!input.trim() || !selectedSpec}
                >
                  <ArrowUp className="size-4" />
                  <span className="sr-only">Send</span>
                </Button>
              )}
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
