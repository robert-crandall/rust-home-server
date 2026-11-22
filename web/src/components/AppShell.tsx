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

  const displayName = () => props.user.name || props.user.email;

  return (
    <div class="page-shell">
      <header class="topbar">
        <div class="topbar-inner">
          <div class="brand">
            <img class="brand-icon" src="/icon.svg" alt="" />
            <span>Rust Home Server</span>
          </div>

          <div class="topbar-actions">
            <span class="user-pill" title={props.user.email}>
              {displayName()}
            </span>
            <ThemePicker />
            <button
              type="button"
              class="btn btn-secondary"
              onClick={() => void handleLogout()}
            >
              Sign out
            </button>
          </div>
        </div>
      </header>

      <main class="main-container">
        <Show when={logoutError()}>
          {(msg) => (
            <div class="alert-error" role="alert">
              {msg()}
            </div>
          )}
        </Show>

        <section class="surface-card" aria-labelledby="focus-heading">
          <span class="eyebrow">Homelab starter</span>
          <h1 id="focus-heading">Ready for Rust Home Server</h1>
          <p class="lead-copy">
            Signed in as <strong>{displayName()}</strong>. Replace this view with
            your application features.
          </p>
        </section>

        <details class="disclosure" onToggle={handleDisclosureToggle}>
          <summary>
            <span>AI &amp; API access</span>
            <span class="disclosure-hint">
              Personal access tokens for rust-home-server-mcp
            </span>
          </summary>

          <div class="disclosure-body">
            <p class="lead-copy">
              Run <code>make mcp-token</code> on your workstation or mint a
              personal access token below for <code>rust-home-server-mcp</code>.
            </p>

            <Show when={tokenError()}>
              {(msg) => (
                <div class="alert-error" role="alert">
                  {msg()}
                </div>
              )}
            </Show>

            <Show when={createdToken()}>
              {(minted) => (
                <div class="secret-banner" role="status">
                  <strong>Copy your new token now (shown once):</strong>
                  <div class="secret-row">
                    <code class="secret-code">{minted().token}</code>
                  </div>
                  <p>
                    Token <strong>{minted().name}</strong> ending in{" "}
                    <code>{minted().last4}</code>.
                  </p>
                </div>
              )}
            </Show>

            <form class="inline-form" onSubmit={(e) => void handleCreateToken(e)}>
              <div class="field">
                <label for="new-token-name">New token name</label>
                <input
                  id="new-token-name"
                  class="input"
                  type="text"
                  placeholder="e.g. laptop-mcp"
                  required
                  maxlength={80}
                  value={tokenName()}
                  onInput={(e) => setTokenName(e.currentTarget.value)}
                />
              </div>
              <button class="btn btn-primary" type="submit" disabled={creating()}>
                {creating() ? "Minting..." : "Mint token"}
              </button>
            </form>

            <Show
              when={tokens().length > 0}
              fallback={
                <Show when={tokensLoaded()}>
                  <p class="field-hint">
                    No personal access tokens minted for this account yet.
                  </p>
                </Show>
              }
            >
              <ul class="token-list" aria-label="Personal access tokens">
                <For each={tokens()}>
                  {(tok) => (
                    <li class="token-item">
                      <div class="token-meta">
                        <span class="token-name">{tok.name}</span>
                        <span class="token-detail">
                          Ends in <code>{tok.last4}</code>
                        </span>
                      </div>
                      <button
                        type="button"
                        class="btn btn-danger"
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
        </details>
      </main>
    </div>
  );
}
