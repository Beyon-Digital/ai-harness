export interface AcpConnector {
  id: string;
  name: string;
  command: string;
  // Whitespace-separated or JSON array — forwarded verbatim to the
  // acp-loop adapter, which parses both forms.
  args?: string;
  cwd?: string;
  timeoutMs?: number;
  allowTools?: boolean;
}

export const CONNECTORS_KEY = "agentos.acp_connectors";

export function loadConnectors(): AcpConnector[] {
  try {
    const raw = localStorage.getItem(CONNECTORS_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as AcpConnector[];
    return Array.isArray(parsed)
      ? parsed.filter((c) => c && c.id && c.command)
      : [];
  } catch {
    return [];
  }
}
