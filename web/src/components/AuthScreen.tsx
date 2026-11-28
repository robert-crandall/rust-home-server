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
    <main class="min-h-screen flex flex-col items-center justify-center p-6">
      <section
        class="card card-border bg-base-100 shadow-sm w-full max-w-sm"
        aria-labelledby="auth-heading"
      >
        <div class="card-body gap-5">
          <header class="flex flex-col gap-2">
            <div class="inline-flex items-center gap-2.5 font-bold text-lg tracking-tight">
              <img class="size-7 rounded-md shrink-0" src="/icon-192.png" alt="" />
              <span>Rust Home Server</span>
            </div>
            <h1 id="auth-heading" class="text-2xl font-bold leading-tight">
              {isRegister() ? "Create account" : "Sign in"}
            </h1>
            <p class="text-sm text-muted">
              {isRegister()
                ? "Create an account to get started."
                : "Sign in to your account."}
            </p>
          </header>

          <Show when={error()}>
            {(msg) => (
              <div class="alert alert-error alert-soft text-sm" role="alert">
                {msg()}
              </div>
            )}
          </Show>

          <Show when={auth.cloudflareAccessEnabled()}>
            <a href="/cdn-cgi/access/login" class="btn btn-primary w-full">
              Sign in with Cloudflare Access
            </a>
          </Show>

          <form class="flex flex-col gap-4" onSubmit={(e) => void handleSubmit(e)}>
            <Show when={isRegister()}>
              <div class="flex flex-col gap-1.5">
                <label class="text-sm font-semibold" for="auth-name">
                  Name (optional)
                </label>
                <input
                  id="auth-name"
                  class="input w-full"
                  type="text"
                  autocomplete="name"
                  value={name()}
                  onInput={(e) => setName(e.currentTarget.value)}
                />
              </div>
            </Show>

            <div class="flex flex-col gap-1.5">
              <label class="text-sm font-semibold" for="auth-email">
                Email
              </label>
              <input
                id="auth-email"
                class="input w-full"
                type="email"
                required
                autocomplete="email"
                value={email()}
                onInput={(e) => setEmail(e.currentTarget.value)}
              />
            </div>

            <div class="flex flex-col gap-1.5">
              <label class="text-sm font-semibold" for="auth-password">
                Password
              </label>
              <input
                id="auth-password"
                class="input w-full"
                type="password"
                required
                minlength={isRegister() ? 8 : 1}
                autocomplete={isRegister() ? "new-password" : "current-password"}
                value={password()}
                onInput={(e) => setPassword(e.currentTarget.value)}
              />
              <Show when={isRegister()}>
                <span class="text-xs text-muted">At least 8 characters.</span>
              </Show>
            </div>

            <button
              class="btn btn-primary w-full"
              type="submit"
              disabled={submitting()}
            >
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
                    class="link link-primary text-sm font-semibold"
                    onClick={() => switchMode("register")}
                  >
                    Need an account? Create one
                  </button>
                }
              >
                <button
                  type="button"
                  class="link link-primary text-sm font-semibold"
                  onClick={() => switchMode("login")}
                >
                  Already have an account? Sign in
                </button>
              </Show>
            </div>
          </Show>
        </div>
      </section>

      <footer class="mt-4 flex items-center justify-between gap-3 w-full max-w-sm text-sm text-muted">
        <span>Self-hosted homelab portal</span>
        <ThemePicker />
      </footer>
    </main>
  );
}
