import { createSignal, For } from "solid-js";
import type { ThemeChoice, ThemePreference } from "../env";

const FALLBACK_CHOICES: ThemeChoice[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

export function ThemePicker() {
  const choices = () => window.appTheme?.choices ?? FALLBACK_CHOICES;
  const [theme, setTheme] = createSignal<ThemePreference>(
    window.appTheme?.get() ?? "system",
  );

  function handleChange(next: ThemePreference) {
    setTheme(next);
    window.appTheme?.set(next);
  }

  return (
    <select
      class="select select-sm w-auto"
      aria-label="Color theme"
      value={theme()}
      onChange={(e) => handleChange(e.currentTarget.value as ThemePreference)}
    >
      <For each={choices()}>
        {(choice) => <option value={choice.value}>{choice.label}</option>}
      </For>
    </select>
  );
}
