import { createSignal, Show } from "solid-js";
import { useAuth } from "../lib/auth";
import { ThemePicker } from "./ThemePicker";

export function AuthScreen() {
  const auth = useAuth();
  const [mode, setMode] = createSignal<"login" | "register">("login");
  const [name, setName] = createSignal("");
  const [email, setEmail] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [submitting, setSubmitting] = createSignal(false);

  function switchMode(next: "login" | "register") {
    setMode(next);
    setError(null);
  }

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);

    try {
      const result = isRegister()
        ? await auth.register({
            email: email().trim(),
            password: password(),
            name: name().trim() ? name().trim() : null,
          })
        : await auth.login({
            email: email().trim(),
            password: password(),
          });

      if (result.error) {
        setError(result.error);
      }
    } catch {
      setError("Could not reach the server.");
    } finally {
      setSubmitting(false);
    }
  }

  const isRegister = () => mode() === "register" && auth.registrationOpen();

  return (
    <main class="auth-container">
      <section class="auth-card" aria-labelledby="auth-heading">
        <header class="auth-header">
          <div class="brand">
            <img class="brand-icon" src="/icon.svg" alt="" />
            <span>Rust Home Template</span>
          </div>
          <h1 id="auth-heading">
            {isRegister() ? "Create household account" : "Sign in"}
          </h1>
          <p>
            {isRegister()
              ? "Create your login to join the shared household workspace."
              : "Sign in to access your household workspace."}
          </p>
        </header>

        <Show when={error()}>
          {(msg) => (
            <div class="alert-error" role="alert">
              {msg()}
            </div>
          )}
        </Show>

        <form class="stack-form" onSubmit={(e) => void handleSubmit(e)}>
          <Show when={isRegister()}>
            <div class="field">
              <label for="auth-name">Name (optional)</label>
              <input
                id="auth-name"
                class="input"
                type="text"
                autocomplete="name"
                value={name()}
                onInput={(e) => setName(e.currentTarget.value)}
              />
            </div>
          </Show>

          <div class="field">
            <label for="auth-email">Email</label>
            <input
              id="auth-email"
              class="input"
              type="email"
              required
              autocomplete="email"
              value={email()}
              onInput={(e) => setEmail(e.currentTarget.value)}
            />
          </div>

          <div class="field">
            <label for="auth-password">Password</label>
            <input
              id="auth-password"
              class="input"
              type="password"
              required
              minlength={isRegister() ? 8 : 1}
              autocomplete={isRegister() ? "new-password" : "current-password"}
              value={password()}
              onInput={(e) => setPassword(e.currentTarget.value)}
            />
            <Show when={isRegister()}>
              <span class="field-hint">At least 8 characters.</span>
            </Show>
          </div>

          <button class="btn btn-primary" type="submit" disabled={submitting()}>
            {submitting()
              ? isRegister()
                ? "Creating account..."
                : "Signing in..."
              : isRegister()
                ? "Create account"
                : "Sign in"}
          </button>
        </form>

        <Show when={auth.registrationOpen()}>
          <div>
            <Show
              when={isRegister()}
              fallback={
                <button
                  type="button"
                  class="link-btn"
                  onClick={() => switchMode("register")}
                >
                  Need a household login? Create an account
                </button>
              }
            >
              <button
                type="button"
                class="link-btn"
                onClick={() => switchMode("login")}
              >
                Already have an account? Sign in
              </button>
            </Show>
          </div>
        </Show>
      </section>

      <footer class="auth-footer">
        <span>Self-hosted household workspace</span>
        <ThemePicker />
      </footer>
    </main>
  );
}
