import { createSignal, Show } from "solid-js";
import type { User } from "../lib/api/client";
import { useAuth } from "../lib/auth";
import { SettingsView } from "./SettingsView";
import { ThemePicker } from "./ThemePicker";

export function AppShell(props: { user: User }) {
  const auth = useAuth();
  const [view, setView] = createSignal<"home" | "settings">("home");
  const [logoutError, setLogoutError] = createSignal<string | null>(null);
  const [failedAvatarUrl, setFailedAvatarUrl] = createSignal<string | null>(null);

  async function handleLogout() {
    setLogoutError(null);
    const result = await auth.logout();
    if (result.error) {
      setLogoutError(result.error);
    }
  }

  const displayName = () => props.user.name || props.user.email;
  const avatarUrl = () =>
    props.user.avatarUrl && failedAvatarUrl() !== props.user.avatarUrl
      ? props.user.avatarUrl
      : null;
  const initials = () => {
    const parts = displayName()
      .trim()
      .split(/[\s@._-]+/)
      .filter(Boolean);
    if (parts.length === 0) return "?";
    if (parts.length === 1) return Array.from(parts[0]).slice(0, 2).join("").toUpperCase();
    return (Array.from(parts[0])[0] + Array.from(parts[parts.length - 1])[0]).toUpperCase();
  };

  return (
    <div class="min-h-screen flex flex-col">
      <header class="navbar bg-base-100 border-b border-base-300 px-0 min-h-0">
        <div class="mx-auto w-full max-w-4xl px-5 py-3 flex flex-wrap items-center justify-between gap-4">
          <button
            type="button"
            class="inline-flex items-center gap-2.5 font-bold text-lg tracking-tight bg-transparent border-0 p-0 text-base-content cursor-pointer rounded-md hover:opacity-80"
            onClick={() => setView("home")}
          >
            <img class="size-7 rounded-md shrink-0" src="/icon.svg" alt="" />
            <span>Rust Home Server</span>
          </button>

          <div class="flex flex-wrap items-center gap-3">
            <button
              type="button"
              class={[
                "user-profile-trigger inline-flex items-center gap-2 rounded-full border py-1 ps-1 pe-3 text-sm text-base-content cursor-pointer transition-colors",
                view() === "settings"
                  ? "bg-primary/10 border-primary/40 font-medium"
                  : "bg-base-200 border-base-300 hover:bg-base-300/60",
              ]}
              title={`Settings (${props.user.email})`}
              aria-label={`Account settings for ${displayName()}`}
              aria-current={view() === "settings" ? "page" : undefined}
              onClick={() => setView("settings")}
            >
              <Show
                when={avatarUrl()}
                fallback={
                  <span
                    class="user-avatar-fallback inline-flex size-6 shrink-0 items-center justify-center rounded-full bg-primary text-primary-content text-[11px] font-bold tracking-wide"
                    aria-hidden="true"
                  >
                    {initials()}
                  </span>
                }
              >
                {(url) => (
                  <img
                    class="user-avatar block size-6 shrink-0 rounded-full object-cover"
                    src={url()}
                    alt=""
                    onError={() => setFailedAvatarUrl(url())}
                  />
                )}
              </Show>
              <span>{displayName()}</span>
            </button>
            <ThemePicker />
            <button
              type="button"
              class="btn btn-outline btn-sm"
              onClick={() => void handleLogout()}
            >
              Sign out
            </button>
          </div>
        </div>
      </header>

      <main class="mx-auto w-full max-w-4xl px-5 pt-8 pb-14 flex flex-col gap-6">
        <Show when={logoutError()}>
          {(msg) => (
            <div class="alert alert-error alert-soft text-sm" role="alert">
              {msg()}
            </div>
          )}
        </Show>

        <Show
          when={view() === "settings"}
          fallback={
            <section
              class="card card-border bg-base-100 shadow-sm"
              aria-labelledby="focus-heading"
            >
              <div class="card-body gap-3">
                <span class="badge badge-soft badge-primary text-xs font-semibold uppercase tracking-wider">
                  Homelab starter
                </span>
                <h1 id="focus-heading" class="text-2xl font-bold tracking-tight">
                  Ready for Rust Home Server
                </h1>
                <p class="text-muted max-w-[62ch]">
                  Signed in as{" "}
                  <strong class="text-base-content">{displayName()}</strong>. Replace
                  this view with your application features.
                </p>
              </div>
            </section>
          }
        >
          <SettingsView
            user={props.user}
            displayName={displayName()}
            initials={initials()}
            avatarUrl={avatarUrl()}
            onAvatarError={setFailedAvatarUrl}
            onBack={() => setView("home")}
          />
        </Show>
      </main>
    </div>
  );
}
