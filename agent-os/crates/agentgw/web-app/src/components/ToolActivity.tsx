import {
  Camera,
  Check,
  ChevronRight,
  Circle,
  Keyboard,
  LoaderCircle,
  Monitor,
  MousePointer2,
  TriangleAlert,
  Wrench,
} from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { decodeDataUri, type Decision, type EffectTrace } from "@/lib/api";
import { cn } from "@/lib/utils";

interface McpPayload {
  op?: string;
  server?: string;
  tool?: string;
  arguments?: Record<string, unknown>;
}

interface McpContent {
  type?: string;
  text?: string;
  mimeType?: string;
  data?: string;
}

function object(value: unknown): Record<string, unknown> {
  return value && typeof value === "object"
    ? (value as Record<string, unknown>)
    : {};
}

function parseJson(value: unknown): Record<string, unknown> {
  if (typeof value !== "string") return object(value);
  try {
    return object(JSON.parse(value));
  } catch {
    return {};
  }
}

function ToolGlyph({
  server,
  tool,
}: {
  server?: string;
  tool?: string;
}) {
  const className = "size-3.5 text-muted-foreground";
  if (server === "computer" && tool === "screenshot")
    return <Camera className={className} />;
  if (server === "computer" && tool === "click")
    return <MousePointer2 className={className} />;
  if (server === "computer" && (tool === "type" || tool === "key"))
    return <Keyboard className={className} />;
  if (server === "computer" && tool === "wait")
    return <Circle className={className} />;
  if (server === "computer") return <Monitor className={className} />;
  return <Wrench className={className} />;
}

function label(payload: McpPayload) {
  const tool = payload.tool || payload.op || "tool";
  if (payload.server === "computer") {
    if (tool === "screenshot") return "Viewed the screen";
    if (tool === "click") return "Clicked the screen";
    if (tool === "type") return "Typed text";
    if (tool === "key") return "Pressed a key";
    if (tool === "wait") return "Waited for the screen";
  }
  return `${payload.server || "tool"} · ${tool}`;
}

function summary(payload: McpPayload) {
  const args = payload.arguments ?? {};
  if (payload.tool === "click")
    return `${String(args.button ?? "left")} at ${String(args.x)}, ${String(args.y)}`;
  if (payload.tool === "type")
    return `"${String(args.text ?? "").slice(0, 80)}"`;
  if (payload.tool === "key") return String(args.key ?? "");
  if (payload.tool === "wait")
    return `${String(args.milliseconds ?? 750)} ms`;
  return "";
}

function resultContent(effect?: EffectTrace) {
  const decoded = decodeDataUri(effect?.result_ref ?? "");
  if (!decoded) return { text: "", image: "" };
  try {
    const value = JSON.parse(decoded) as { content?: McpContent[] };
    const content = Array.isArray(value.content) ? value.content : [];
    const text = content
      .filter((item) => item.type === "text")
      .map((item) => item.text)
      .filter(Boolean)
      .join("\n");
    const image = content.find(
      (item) => item.type === "image" && item.mimeType && item.data,
    );
    return {
      text,
      image: image
        ? `data:${image.mimeType};base64,${image.data}`
        : "",
    };
  } catch {
    return { text: decoded, image: "" };
  }
}

function EffectStatus({ state }: { state?: string }) {
  if (state === "committed")
    return <Check className="size-3.5 text-emerald-500" />;
  if (state === "failed" || state === "cancelled")
    return <TriangleAlert className="size-3.5 text-destructive" />;
  return <LoaderCircle className="size-3.5 animate-spin text-muted-foreground" />;
}

function ToolCard({ decision }: { decision: Decision }) {
  const detail = decision.detail ?? {};
  const payload = parseJson(detail.payload) as McpPayload;
  const effect = object(detail.effect) as unknown as EffectTrace;
  const result = resultContent(effect);
  const failed = effect?.state === "failed" || effect?.state === "cancelled";

  return (
    <Collapsible defaultOpen={Boolean(result.image || failed)}>
      <div
        className={cn(
          "overflow-hidden rounded-lg border bg-card/50 text-xs",
          failed && "border-destructive/40",
        )}
      >
        <CollapsibleTrigger className="group flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-muted/50">
          <EffectStatus state={effect?.state} />
          <ToolGlyph server={payload.server} tool={payload.tool} />
          <span className="font-medium">{label(payload)}</span>
          <span className="min-w-0 flex-1 truncate text-muted-foreground">
            {summary(payload)}
          </span>
          <ChevronRight className="size-3.5 text-muted-foreground transition-transform group-data-panel-open:rotate-90" />
        </CollapsibleTrigger>
        <CollapsibleContent>
          <div className="space-y-2 border-t px-3 py-2">
            {result.image && (
              <img
                src={result.image}
                alt="Computer tool screenshot"
                className="max-h-80 w-full rounded-md border object-contain bg-black"
              />
            )}
            {result.text && (
              <p className="whitespace-pre-wrap text-muted-foreground">
                {result.text}
              </p>
            )}
            {failed && (
              <p className="text-destructive">
                {effect?.error_code || "Tool call failed"}
              </p>
            )}
            {payload.arguments && Object.keys(payload.arguments).length > 0 && (
              <pre className="max-h-40 overflow-auto rounded-md bg-muted/60 p-2 font-mono text-[11px] text-muted-foreground">
                {JSON.stringify(payload.arguments, null, 2)}
              </pre>
            )}
          </div>
        </CollapsibleContent>
      </div>
    </Collapsible>
  );
}

export function ToolActivity({ decisions }: { decisions: Decision[] }) {
  const tools = decisions.filter((decision) => {
    if (decision.kind !== "invoke_effect") return false;
    const payload = parseJson(decision.detail?.payload);
    return String(payload.op ?? "").startsWith("mcp.");
  });
  const modelCalls = decisions.filter(
    (decision) =>
      decision.kind === "invoke_effect" &&
      decision.detail?.operation === "model.chat",
  ).length;

  if (!tools.length && modelCalls < 2) return null;

  return (
    <div className="mt-3 space-y-1.5">
      {tools.map((decision) => (
        <ToolCard key={decision.sequence} decision={decision} />
      ))}
      {modelCalls > 1 && (
        <p className="px-1 text-[11px] text-muted-foreground">
          {modelCalls} model turns
        </p>
      )}
    </div>
  );
}
