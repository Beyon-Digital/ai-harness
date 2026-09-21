export interface Provider {
  id: string;
  name: string;
  baseUrl: string;
  model: string;
  apiKey?: string;
  builtin?: boolean;
}

export const PROVIDERS_KEY = "agentos.providers";

export function defaultProviders(): Provider[] {
  return [
    {
      id: "openrouter",
      name: "OpenRouter",
      baseUrl: "https://openrouter.ai/api/v1",
      model: "meta-llama/llama-3.3-70b-instruct:free",
      builtin: true,
    },
  ];
}

export function loadProviders(): Provider[] {
  try {
    const raw = localStorage.getItem(PROVIDERS_KEY);
    if (!raw) return defaultProviders();
    const parsed = JSON.parse(raw) as Provider[];
    return parsed.length ? parsed : defaultProviders();
  } catch {
    return defaultProviders();
  }
}
