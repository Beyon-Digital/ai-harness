import { useSyncExternalStore } from "react";

export interface Theme {
  id: string;
  label: string;
  /** Light | dark — drives `dark:` utilities, sonner, React Flow. */
  mode: "light" | "dark";
}

export const THEMES: Theme[] = [
  { id: "default", label: "Agent OS (light)", mode: "light" },
  { id: "default-dark", label: "Agent OS (dark)", mode: "dark" },
  { id: "github", label: "GitHub (light)", mode: "light" },
  { id: "github-dark", label: "GitHub (dark)", mode: "dark" },
  { id: "dracula", label: "Dracula", mode: "dark" },
  { id: "vscode", label: "VS Code (light)", mode: "light" },
  { id: "vscode-dark", label: "VS Code (dark)", mode: "dark" },
];

export const THEME_KEY = "agentos.theme";

export function loadThemeId(): string {
  try {
    const id = localStorage.getItem(THEME_KEY);
    return THEMES.some((t) => t.id === id) ? id! : "default";
  } catch {
    return "default";
  }
}

export function themeById(id: string): Theme {
  return THEMES.find((t) => t.id === id) ?? THEMES[0];
}

let current = loadThemeId();
const listeners = new Set<() => void>();

export function applyTheme(id: string) {
  const t = themeById(id);
  current = t.id;
  const root = document.documentElement;
  root.classList.toggle("dark", t.mode === "dark");
  root.setAttribute("data-theme", t.id === "default" ? "" : t.id);
  try {
    localStorage.setItem(THEME_KEY, t.id);
  } catch {
    /* private mode — theme just won't persist */
  }
  listeners.forEach((f) => f());
}

export function useTheme(): Theme {
  return themeById(
    useSyncExternalStore(
      (f) => {
        listeners.add(f);
        return () => listeners.delete(f);
      },
      () => current,
    ),
  );
}

/** Call before first render so the app boots in the saved theme. */
applyTheme(current);
