import { createSignal, For, onSettled, Show } from "solid-js";
import { api, type ApiToken, type CreatedApiToken, type User } from "../lib/api/client";

export function SettingsView(props: {
  user: User;
  displayName: string;
  initials: string;
  avatarUrl: string | null;
  onAvatarError: (url: string) => void;
  onBack: () => void;
}) {
  const [tokensLoaded, setTokensLoaded] = createSignal(false);
  const [tokens, setTokens] = createSignal<ApiToken[]>([]);
  const [tokenName, setTokenName] = createSignal("");
  const [createdToken, setCreatedToken] = createSignal<CreatedApiToken | null>(null);
  const [copyState, setCopyState] = createSignal<{
    tokenId: string;
    status: "copied" | "failed";
  } | null>(null);
  const [tokenError, setTokenError] = createSignal<string | null>(null);
  const [creating, setCreating] = createSignal(false);
  const canCopy = () =>
    typeof navigator !== "undefined" && Boolean(navigator.clipboard?.writeText);
  let tokenLoadSeq = 0;
  let mounted = true;

  async function loadTokens() {
    const seq = ++tokenLoadSeq;
    setTokenError(null);
    try {
      const { data, error } = await api.GET("/api/tokens");
      if (!mounted || seq !== tokenLoadSeq) return;
      if (error || !data) {
        setTokenError(error?.error ?? "Could not load personal access tokens.");
        return;
      }
      setTokens(data);
      setTokensLoaded(true);
    } catch {
      if (!mounted || seq !== tokenLoadSeq) return;
      setTokenError("Could not reach the server to load personal access tokens.");
    }
  }

  onSettled(() => {
    void loadTokens();
    return () => {
      mounted = false;
      ++tokenLoadSeq;
    };
  });

  async function handleCreateToken(e: SubmitEvent) {
    e.preventDefault();
    const trimmed = tokenName().trim();
    if (!trimmed) return;

    setCreating(true);
    setTokenError(null);
    setCopyState(null);
    try {
      const { data, error } = await api.POST("/api/tokens", {
        body: { name: trimmed },
      });
      if (!mounted) return;
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
      if (!mounted) return;
      setTokenError(
        "Could not reach the server while minting the token. Refresh tokens to check existing tokens before retrying.",
      );
    } finally {
      if (mounted) {
        setCreating(false);
      }
    }
  }

  async function handleRevokeToken(id: string) {
    setTokenError(null);
    try {
      const { error } = await api.DELETE("/api/tokens/{id}", {
        params: { path: { id } },
      });
      if (!mounted) return;
      if (error) {
        setTokenError(error.error);
        return;
      }
      ++tokenLoadSeq;
      setTokens((prev) => prev.filter((item) => item.id !== id));
      if (createdToken()?.id === id) {
        setCreatedToken(null);
        setCopyState(null);
      }
    } catch {
      if (!mounted) return;
      setTokenError("Could not reach the server to revoke that token.");
    }
  }

  async function handleCopyToken(minted: CreatedApiToken) {
    if (!canCopy()) return;
    try {
      await navigator.clipboard.writeText(minted.token);
      if (mounted) {
        setCopyState({ tokenId: minted.id, status: "copied" });
      }
    } catch {
      if (mounted) {
        setCopyState({ tokenId: minted.id, status: "failed" });
      }
    }
  }

  return (
    <div class="flex flex-col gap-6">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <h1 class="text-2xl font-bold tracking-tight">Settings</h1>
        <button
          type="button"
          class="btn btn-outline btn-sm"
          onClick={() => props.onBack()}
        >
          Back
        </button>
      </div>

      <section
        class="card card-border bg-base-100 shadow-sm"
        aria-labelledby="account-heading"
      >
        <div class="card-body gap-4">
          <h2 id="account-heading" class="text-base font-semibold">
            Account
          </h2>
          <div class="flex items-center gap-3.5">
            <Show
              when={props.avatarUrl}
              fallback={
                <span
                  class="inline-flex size-10 shrink-0 items-center justify-center rounded-full bg-primary text-primary-content text-sm font-bold tracking-wide"
                  aria-hidden="true"
                >
                  {props.initials}
                </span>
              }
            >
              {(url) => (
                <img
                  class="block size-10 shrink-0 rounded-full object-cover"
                  src={url()}
                  alt=""
                  onError={() => props.onAvatarError(url())}
                />
              )}
            </Show>
            <div class="flex flex-col min-w-0">
              <span class="font-semibold truncate">{props.displayName}</span>
              <span class="text-sm text-muted truncate">{props.user.email}</span>
            </div>
          </div>
        </div>
      </section>

      <section
        class="card card-border bg-base-100 shadow-sm"
        aria-labelledby="api-tokens-heading"
      >
        <div class="card-body gap-5">
          <div class="flex flex-wrap items-start justify-between gap-3">
            <div class="flex flex-col gap-1">
              <h2 id="api-tokens-heading" class="text-base font-semibold">
                AI &amp; API access
              </h2>
              <p class="text-sm text-muted">
                Personal access tokens for <code class="text-xs">rust-home-server-mcp</code>
              </p>
            </div>
            <button
              type="button"
              class="btn btn-outline btn-xs"
              onClick={() => void loadTokens()}
            >
              Refresh
            </button>
          </div>

          <p class="text-sm text-muted max-w-[62ch]">
            Run <code class="text-xs">make mcp-token</code> on your
            workstation or mint a personal access token below for{" "}
            <code class="text-xs">rust-home-server-mcp</code>.
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
                  <Show when={canCopy()}>
                    <button
                      type="button"
                      class="btn btn-outline btn-sm"
                      onClick={() => void handleCopyToken(minted())}
                    >
                      {copyState()?.tokenId === minted().id &&
                      copyState()?.status === "copied"
                        ? "Copied"
                        : "Copy"}
                    </button>
                  </Show>
                </div>
                <Show
                  when={
                    copyState()?.tokenId === minted().id &&
                    copyState()?.status === "failed"
                  }
                >
                  <p class="text-xs text-error" role="alert">
                    Copy failed - select and copy the token manually.
                  </p>
                </Show>
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
      </section>
    </div>
  );
}
