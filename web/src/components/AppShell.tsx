import { createSignal, For, Show } from "solid-js";
import { api, type ApiToken, type CreatedApiToken, type User } from "../lib/api/client";
import { useAuth } from "../lib/auth";
import { ThemePicker } from "./ThemePicker";

export function AppShell(props: { user: User }) {
  const auth = useAuth();
  const [tokensLoaded, setTokensLoaded] = createSignal(false);
  const [tokens, setTokens] = createSignal<ApiToken[]>([]);
  const [tokenName, setTokenName] = createSignal("");
  const [createdToken, setCreatedToken] = createSignal<CreatedApiToken | null>(null);
  const [tokenError, setTokenError] = createSignal<string | null>(null);
  const [logoutError, setLogoutError] = createSignal<string | null>(null);
  const [creating, setCreating] = createSignal(false);
  let tokenLoadSeq = 0;

  async function handleLogout() {
    setLogoutError(null);
    const result = await auth.logout();
    if (result.error) {
      setLogoutError(result.error);
    }
  }

  async function loadTokens() {
    const seq = ++tokenLoadSeq;
    setTokenError(null);
    try {
      const { data, error } = await api.GET("/api/tokens");
      if (seq !== tokenLoadSeq) return;
      if (error || !data) {
        setTokenError(error?.error ?? "Could not load personal access tokens.");
        return;
      }
      setTokens(data);
      setTokensLoaded(true);
    } catch {
      if (seq !== tokenLoadSeq) return;
      setTokenError("Could not reach the server to load personal access tokens.");
    }
  }

  function handleDisclosureToggle(e: Event & { currentTarget: HTMLDetailsElement }) {
    if (e.currentTarget.open) {
      void loadTokens();
    }
  }

  async function handleCreateToken(e: SubmitEvent) {
    e.preventDefault();
    const trimmed = tokenName().trim();
    if (!trimmed) return;

    setCreating(true);
    setTokenError(null);
    try {
      const { data, error } = await api.POST("/api/tokens", {
        body: { name: trimmed },
      });
      if (error || !data) {
        setTokenError(error?.error ?? "Could not create token.");
        return;
      }
      setCreatedToken(data);
      setTokenName("");
      setTokens((prev) => [
        {
          id: data.id,
          name: data.name,
          last4: data.last4,
          createdAt: data.createdAt,
          lastUsedAt: null,
        },
        ...prev.filter((item) => item.id !== data.id),
      ]);
      setTokensLoaded(true);
      await loadTokens();
    } catch {
      setTokenError(
        "Could not reach the server while minting the token. Reopen this panel to check existing tokens before retrying.",
      );
    } finally {
      setCreating(false);
    }
  }

  async function handleRevokeToken(id: string) {
    setTokenError(null);
    try {
      const { error } = await api.DELETE("/api/tokens/{id}", {
        params: { path: { id } },
      });
      if (error) {
        setTokenError(error.error);
        return;
      }
      ++tokenLoadSeq;
      setTokens((prev) => prev.filter((item) => item.id !== id));
      if (createdToken()?.id === id) {
        setCreatedToken(null);
      }
    } catch {
      setTokenError("Could not reach the server to revoke that token.");
    }
  }

  const [failedAvatarUrl, setFailedAvatarUrl] = createSignal<string | null>(null);

  const displayName = () => props.user.name || props.user.email;
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
          <div class="inline-flex items-center gap-2.5 font-bold text-lg tracking-tight">
            <img class="size-7 rounded-md shrink-0" src="/icon.svg" alt="" />
            <span>Rust Home Server</span>
          </div>

          <div class="flex flex-wrap items-center gap-3">
            <span
              class="inline-flex items-center gap-2 rounded-full bg-base-200 border border-base-300 py-1 ps-1 pe-3 text-sm text-base-content"
              title={props.user.email}
            >
              <Show
                when={
                  props.user.avatarUrl && failedAvatarUrl() !== props.user.avatarUrl
                    ? props.user.avatarUrl
                    : null
                }
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
            </span>
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

        <details
          class="collapse collapse-arrow bg-base-100 border border-base-300 shadow-sm"
          onToggle={handleDisclosureToggle}
        >
          <summary class="collapse-title font-semibold">
            <span class="flex flex-wrap items-center justify-between gap-2">
              <span>AI &amp; API access</span>
              <span class="text-sm font-normal text-muted">
                Personal access tokens for rust-home-server-mcp
              </span>
            </span>
          </summary>

          <div class="collapse-content">
            <div class="border-t border-base-300 pt-4 flex flex-col gap-5">
              <p class="text-muted max-w-[62ch]">
                Run <code class="text-sm">make mcp-token</code> on your
                workstation or mint a personal access token below for{" "}
                <code class="text-sm">rust-home-server-mcp</code>.
              </p>

              <Show when={tokenError()}>
                {(msg) => (
                  <div class="alert alert-error alert-soft text-sm" role="alert">
                    {msg()}
                  </div>
                )}
              </Show>

              <Show when={createdToken()}>
                {(minted) => (
                  <div
                    class="rounded-box border border-primary/25 bg-primary/10 p-4 flex flex-col gap-2"
                    role="status"
                  >
                    <strong>Copy your new token now (shown once):</strong>
                    <div class="flex flex-wrap items-center gap-2.5">
                      <code class="flex-1 basis-64 rounded-field border border-base-300 bg-base-100 px-2.5 py-2 text-sm overflow-x-auto break-all">
                        {minted().token}
                      </code>
                    </div>
                    <p class="text-sm text-muted">
                      Token{" "}
                      <strong class="text-base-content">{minted().name}</strong>{" "}
                      ending in <code class="text-sm">{minted().last4}</code>.
                    </p>
                  </div>
                )}
              </Show>

              <form
                class="flex flex-wrap items-end gap-3"
                onSubmit={(e) => void handleCreateToken(e)}
              >
                <div class="flex flex-col gap-1.5 flex-1 basis-56">
                  <label class="text-sm font-semibold" for="new-token-name">
                    New token name
                  </label>
                  <input
                    id="new-token-name"
                    class="input w-full"
                    type="text"
                    placeholder="e.g. laptop-mcp"
                    required
                    maxlength={80}
                    value={tokenName()}
                    onInput={(e) => setTokenName(e.currentTarget.value)}
                  />
                </div>
                <button
                  class="btn btn-primary"
                  type="submit"
                  disabled={creating()}
                >
                  {creating() ? "Minting..." : "Mint token"}
                </button>
              </form>

              <Show
                when={tokens().length > 0}
                fallback={
                  <Show when={tokensLoaded()}>
                    <p class="text-xs text-muted">
                      No personal access tokens minted for this account yet.
                    </p>
                  </Show>
                }
              >
                <ul
                  class="divide-y divide-base-300 border-y border-base-300"
                  aria-label="Personal access tokens"
                >
                  <For each={tokens()}>
                    {(tok) => (
                      <li class="token-item flex flex-wrap items-center justify-between gap-4 py-3">
                        <div class="flex flex-col gap-0.5">
                          <span class="token-name font-semibold text-sm">
                            {tok.name}
                          </span>
                          <span class="text-xs text-muted">
                            Ends in <code class="text-xs">{tok.last4}</code>
                          </span>
                        </div>
                        <button
                          type="button"
                          class="btn btn-error btn-outline btn-sm"
                          onClick={() => void handleRevokeToken(tok.id)}
                        >
                          Revoke
                        </button>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </div>
          </div>
        </details>
      </main>
    </div>
  );
}
