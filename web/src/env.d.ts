export type ThemePreference = "system" | "light" | "dark";

export interface ThemeChoice {
  value: ThemePreference;
  label: string;
}

declare global {
  interface Window {
    appTheme?: {
      choices: ThemeChoice[];
      get: () => ThemePreference;
      set: (value: ThemePreference) => void;
    };
  }
}
