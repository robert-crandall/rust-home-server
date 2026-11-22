// Blocking head script so a saved theme applies before first paint (no FOUC).
// Components read and update the active theme through window.appTheme.
(() => {
  const choices = [
    { value: "system", label: "System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];
  let preference = "system";

  function apply() {
    if (preference === "system") {
      document.documentElement.removeAttribute("data-theme");
    } else {
      document.documentElement.dataset.theme = preference;
    }
  }

  try {
    const saved = localStorage.getItem("theme");
    if (saved && choices.some((choice) => choice.value === saved)) {
      preference = saved;
    }
  } catch {
    // Ignore storage errors in restricted browsing modes.
  }
  apply();

  window.appTheme = {
    choices,
    get: () => preference,
    set(value) {
      if (!choices.some((choice) => choice.value === value)) {
        throw new Error(`Unknown theme: ${value}`);
      }
      preference = value;
      apply();
      try {
        if (value === "system") {
          localStorage.removeItem("theme");
        } else {
          localStorage.setItem("theme", value);
        }
      } catch {
        // Theme still applies for the current document lifetime.
      }
    },
  };
})();
