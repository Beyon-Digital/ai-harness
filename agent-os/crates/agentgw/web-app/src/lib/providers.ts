export interface Provider {
  id: string;
  name: string;
  baseUrl: string;
  model: string;
  // Name of a daemon env var holding the key — secrets never live in
  // localStorage or durable run payloads.
  keyEnv?: string;
  builtin?: boolean;
}

export const PROVIDERS_KEY = "agentos.providers";
const LEGACY_OPENROUTER_MODEL = "meta-llama/llama-3.3-70b-instruct:free";

export function defaultProviders(): Provider[] {
  return [
    {
      id: "openrouter",
      name: "OpenRouter",
      baseUrl: "https://openrouter.ai/api/v1",
      model: "openrouter/free",
      builtin: true,
    },
  ];
}

export function loadProviders(): Provider[] {
  try {
    const raw = localStorage.getItem(PROVIDERS_KEY);
    if (!raw) return defaultProviders();
    const parsed = JSON.parse(raw) as Provider[];
    return parsed.length
      ? parsed.map((provider) =>
          provider.id === "openrouter" &&
          provider.builtin &&
          provider.model === LEGACY_OPENROUTER_MODEL
            ? { ...provider, model: "openrouter/free" }
            : provider,
        )
      : defaultProviders();
  } catch {
    return defaultProviders();
  }
}
