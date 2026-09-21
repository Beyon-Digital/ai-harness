import { useCallback, useEffect, useRef, useState } from "react";
import { Bot, ExternalLink, Plus } from "lucide-react";
import { toast } from "sonner";
import {
  Conversation,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { Message, MessageContent } from "@/components/ai-elements/message";
import {
  ChainOfThought,
  ChainOfThoughtContent,
  ChainOfThoughtHeader,
  ChainOfThoughtStep,
} from "@/components/ai-elements/chain-of-thought";
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
import { Badge } from "@/components/ui/badge";
import { Textarea } from "@/components/ui/textarea";
import {
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

interface ChatMsg {
  id: string;
  role: "user" | "assistant";
  text: string;
  runId?: string;
  state?: string;
  decisions?: Decision[];
  error?: boolean;
}

export function ChatPage({ onOpenRun }: { onOpenRun: (runId: string) => void }) {
  const [specs, setSpecs] = useState<Spec[]>([]);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [specId, setSpecId] = useState("");
  const [sessionId, setSessionId] = useState("");
  const [messages, setMessages] = useState<ChatMsg[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [providers] = useState<Provider[]>(loadProviders);
  const [providerId, setProviderId] = useState("default");
  const [newAgentOpen, setNewAgentOpen] = useState(false);
  const [agentName, setAgentName] = useState("");
  const [agentProfile, setAgentProfile] = useState("");
  const [agentDesc, setAgentDesc] = useState("");
  const pollRef = useRef<ReturnType<typeof setInterval>>(undefined);

  const refreshSpecs = useCallback(async () => {
    try {
      const [idx, profs] = await Promise.all([getIndex(), getProfiles()]);
      setSpecs(idx.specs);
      setProfiles(profs);
      if (!specId && idx.specs.length)
        setSpecId(`${idx.specs[0].agent_spec_id}@${idx.specs[0].version}`);
      if (!agentProfile && profs.length) setAgentProfile(profs[0].name);
    } catch (e) {
      toast.error((e as Error).message);
    }
  }, [specId, agentProfile]);

  useEffect(() => {
    refreshSpecs();
    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const ensureSession = async () => {
    if (sessionId) return sessionId;
    const s = await createSession(uuidv7(), "chat session");
    setSessionId(s.session_id);
    return s.session_id;
  };

  const pollRun = useCallback((runId: string, msgId: string) => {
    const tick = async () => {
      try {
        const [run, decisions] = await Promise.all([
          getRun(runId),
          getDecisions(runId).catch(() => [] as Decision[]),
        ]);
        setMessages((ms) =>
          ms.map((m) =>
            m.id === msgId
              ? {
                  ...m,
                  state: run.state_name,
                  decisions,
                  text:
                    decodeDataUri(run.output_ref) ||
                    (run.state_name === "completed"
                      ? "(no text output)"
                      : m.text),
                }
              : m,
          ),
        );
        if (run.state >= 9 && pollRef.current) {
          clearInterval(pollRef.current);
          pollRef.current = undefined;
          setBusy(false);
        }
      } catch {
        /* transient — keep polling */
      }
    };
    pollRef.current = setInterval(tick, 1200);
    tick();
  }, []);

  const send = async () => {
    const task = input.trim();
    if (!task || busy) return;
    if (!specId) return toast.error("create or pick an agent first");
    const [id, version] = specId.split("@");
    const spec = specs.find(
      (s) => s.agent_spec_id === id && s.version === version,
    );
    setBusy(true);
    setInput("");
    const userMsg: ChatMsg = { id: uuidv7(), role: "user", text: task };
    const asstId = uuidv7();
    setMessages((ms) => [
      ...ms,
      userMsg,
      { id: asstId, role: "assistant", text: "thinking…", state: "created" },
    ]);
    try {
      const sid = await ensureSession();
      const provider = providers.find((p) => p.id === providerId);
      // Payload envelope understood by openrouter-loop: plain text is a
      // bare task; an object can pin model / base_url per request.
      const payload = provider
        ? JSON.stringify({
            task,
            model: provider.model,
            base_url: provider.baseUrl,
          })
        : task;
      const { run_id } = await createRun({
        sessionId: sid,
        specId: id,
        specVersion: version,
        specDigest: spec?.digest || "",
        taskPayload: payload,
        profile: spec?.profile,
      });
      setMessages((ms) =>
        ms.map((m) => (m.id === asstId ? { ...m, runId: run_id } : m)),
      );
      pollRun(run_id, asstId);
    } catch (e) {
      setBusy(false);
      setMessages((ms) =>
        ms.map((m) =>
          m.id === asstId
            ? { ...m, text: (e as Error).message, error: true, state: "failed" }
            : m,
        ),
      );
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
          description: agentDesc,
        }),
      );
      toast.success(`agent "${agentName}" created`);
      setNewAgentOpen(false);
      setAgentName("");
      setAgentDesc("");
      refreshSpecs();
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  return (
    <div className="flex h-full">
      <div className="flex w-60 flex-col border-r">
        <div className="flex items-center justify-between border-b p-3">
          <span className="text-sm font-medium">Agents</span>
          <Dialog open={newAgentOpen} onOpenChange={setNewAgentOpen}>
            <DialogTrigger
              render={
                <Button size="icon" variant="ghost" className="size-7">
                  <Plus className="size-4" />
                </Button>
              }
            />
            <DialogContent>
              <DialogHeader>
                <DialogTitle>New agent</DialogTitle>
              </DialogHeader>
              <div className="space-y-3">
                <div>
                  <Label>Name</Label>
                  <Input
                    value={agentName}
                    onChange={(e) => setAgentName(e.target.value)}
                    placeholder="e.g. research-assistant"
                  />
                </div>
                <div>
                  <Label>Runtime profile</Label>
                  <Select
                    value={agentProfile}
                    onValueChange={(v) => v && setAgentProfile(v)}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="pick a profile" />
                    </SelectTrigger>
                    <SelectContent>
                      {profiles.map((p) => (
                        <SelectItem key={p.name} value={p.name}>
                          {p.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <p className="mt-1 text-xs text-muted-foreground">
                    The profile pins which adapters this agent uses (loop,
                    effects, memory, sandbox). Manage profiles on the Pipelines
                    page.
                  </p>
                </div>
                <div>
                  <Label>Description</Label>
                  <Textarea
                    value={agentDesc}
                    onChange={(e) => setAgentDesc(e.target.value)}
                    placeholder="what this agent is for"
                  />
                </div>
              </div>
              <DialogFooter>
                <Button onClick={createAgent}>Create agent</Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </div>
        <div className="flex-1 space-y-1 overflow-y-auto p-2">
          {specs.map((s) => {
            const key = `${s.agent_spec_id}@${s.version}`;
            return (
              <button
                key={key}
                onClick={() => setSpecId(key)}
                className={cn(
                  "w-full rounded-md px-3 py-2 text-left text-sm",
                  specId === key ? "bg-accent" : "hover:bg-accent/50",
                )}
              >
                <div className="flex items-center gap-2 font-medium">
                  <Bot className="size-3.5" />
                  {s.display_name || "agent"}
                </div>
                <div className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
                  {s.agent_spec_id.slice(0, 13)}… · v{s.version}
                </div>
              </button>
            );
          })}
          {!specs.length && (
            <p className="p-3 text-xs text-muted-foreground">
              No agents yet — create one with +
            </p>
          )}
        </div>
      </div>

      <div className="flex min-w-0 flex-1 flex-col">
        <Conversation className="flex-1">
          <ConversationContent>
            {!messages.length && (
              <ConversationEmptyState
                title="Talk to an agent"
                description="Pick an agent on the left (or create one), then send a task — it runs on the kernel with a full decision trail."
              />
            )}
            {messages.map((m) => (
              <Message key={m.id} from={m.role}>
                <MessageContent
                  className={cn(m.error && "border-destructive/50 text-destructive")}
                >
                  <div className="whitespace-pre-wrap">{m.text}</div>
                  {m.state && m.role === "assistant" && (
                    <div className="mt-2 flex items-center gap-2">
                      <Badge
                        variant={
                          m.state === "completed"
                            ? "default"
                            : m.error || m.state === "failed"
                              ? "destructive"
                              : "secondary"
                        }
                      >
                        {m.state}
                      </Badge>
                      {m.runId && (
                        <button
                          className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
                          onClick={() => onOpenRun(m.runId!)}
                        >
                          run <ExternalLink className="size-3" />
                        </button>
                      )}
                    </div>
                  )}
                  {!!m.decisions?.length && (
                    <ChainOfThought className="mt-3">
                      <ChainOfThoughtHeader>
                        decision trail ({m.decisions.length})
                      </ChainOfThoughtHeader>
                      <ChainOfThoughtContent>
                        {m.decisions.map((d) => (
                          <ChainOfThoughtStep
                            key={d.sequence}
                            label={`step ${d.step_sequence} · ${d.kind}`}
                            description={
                              d.detail?.operation ||
                              d.detail?.output_ref?.slice(0, 60) ||
                              Object.values(d.detail || {})[0]?.slice(0, 80)
                            }
                            status={
                              d.kind === "complete"
                                ? "complete"
                                : d.kind === "fail"
                                  ? "complete"
                                  : "active"
                            }
                          />
                        ))}
                      </ChainOfThoughtContent>
                    </ChainOfThought>
                  )}
                </MessageContent>
              </Message>
            ))}
          </ConversationContent>
          <ConversationScrollButton />
        </Conversation>

        <div className="border-t p-3">
          <div className="flex items-end gap-2">
            <Select
              value={providerId}
              onValueChange={(v) => v && setProviderId(v)}
            >
              <SelectTrigger className="w-48">
                <SelectValue placeholder="provider" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="default">default provider</SelectItem>
                {providers.map((p) => (
                  <SelectItem key={p.id} value={p.id}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Textarea
              className="min-h-11 flex-1 resize-none"
              placeholder={
                specId ? "send a task to the agent…" : "create an agent first"
              }
              value={input}
              disabled={!specId}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  send();
                }
              }}
            />
            <Button onClick={send} disabled={busy || !input.trim() || !specId}>
              Send
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
