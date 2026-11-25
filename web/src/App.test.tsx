import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type { ApiToken, User } from "./lib/api/client";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

async function waitFor(assertion: () => void, timeoutMs = 1500): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;
  while (Date.now() < deadline) {
    flush();
    try {
      assertion();
      return;
    } catch (err) {
      lastError = err;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  }
  throw lastError;
}

function setInputValue(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  flush();
}

describe("Rust Home Server SPA foundation", () => {
  let currentUser: User | null;
  let registrationOpen: boolean;
  let cloudflareAccessEnabled: boolean;
  let logoutUrl: string | null;
  let tokens: ApiToken[];
  let tokenLoads: number;
  let failAppBootstrap: boolean;
  let failLoginNetwork: boolean;
  let failTokenLoad: boolean;
  let pendingTokenLoad: ((res: Response) => void) | null;
  let dropCreatedTokenResponse: boolean;
  let failLogoutStatus: boolean;
  let dispose: (() => void) | undefined;

  beforeEach(() => {
    currentUser = null;
    registrationOpen = true;
    cloudflareAccessEnabled = false;
    logoutUrl = null;
    tokens = [];
    tokenLoads = 0;
    failAppBootstrap = false;
    failLoginNetwork = false;
    failTokenLoad = false;
    pendingTokenLoad = null;
    dropCreatedTokenResponse = false;
    failLogoutStatus = false;

    let currentTheme: "system" | "light" | "dark" = "system";
    window.appTheme = {
      choices: [
        { value: "system", label: "System" },
        { value: "light", label: "Light" },
        { value: "dark", label: "Dark" },
      ],
      get: () => currentTheme,
      set: (val) => {
        currentTheme = val;
        if (val === "system") {
          document.documentElement.removeAttribute("data-theme");
        } else {
          document.documentElement.dataset.theme = val;
        }
      },
    };

    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const req = input instanceof Request ? input : new Request(String(input), init);
      const url = new URL(req.url);
      const method = req.method.toUpperCase();

      if (url.pathname === "/api/app" && method === "GET") {
        if (failAppBootstrap) {
          throw new TypeError("Failed to fetch");
        }
        return jsonResponse({ registrationOpen, cloudflareAccessEnabled });
      }

      if (url.pathname === "/api/auth/me" && method === "GET") {
        if (!currentUser) {
          return jsonResponse({ error: "authentication required" }, 401);
        }
        return jsonResponse(currentUser);
      }

      if (url.pathname === "/api/auth/login" && method === "POST") {
        if (failLoginNetwork) {
          throw new TypeError("Failed to fetch");
        }
        const body = (await req.json()) as { email: string; password: string };
        if (body.password !== "correct-horse-battery") {
          return jsonResponse({ error: "invalid email or password" }, 401);
        }
        currentUser = {
          id: "00000000-0000-0000-0000-000000000001",
          email: body.email.toLowerCase(),
          name: "Leo",
          createdAt: "2026-04-20T00:00:00Z",
        };
        return jsonResponse(currentUser);
      }

      if (url.pathname === "/api/auth/register" && method === "POST") {
        const body = (await req.json()) as {
          email: string;
          password: string;
          name?: string | null;
        };
        currentUser = {
          id: "00000000-0000-0000-0000-000000000002",
          email: body.email.toLowerCase(),
          name: body.name ?? "",
          createdAt: "2026-04-20T00:00:00Z",
        };
        return jsonResponse(currentUser, 201);
      }

      if (url.pathname === "/api/auth/logout" && method === "POST") {
        if (failLogoutStatus) {
          return jsonResponse({ error: "session store unavailable" }, 500);
        }
        currentUser = null;
        return jsonResponse({ logoutUrl });
      }

      if (url.pathname === "/api/tokens" && method === "GET") {
        tokenLoads += 1;
        if (pendingTokenLoad) {
          const resolver = pendingTokenLoad;
          pendingTokenLoad = null;
          return new Promise<Response>((resolve) => {
            pendingTokenLoad = resolve;
            void resolver;
          });
        }
        if (failTokenLoad) {
          return jsonResponse({ error: "database unavailable" }, 500);
        }
        return jsonResponse(tokens);
      }

      if (url.pathname === "/api/tokens" && method === "POST") {
        const body = (await req.json()) as { name: string };
        const created = {
          id: "00000000-0000-0000-0000-000000000099",
          name: body.name,
          last4: "a1b2",
          createdAt: "2026-04-20T00:00:00Z",
          token: "pat_00000000-0000-0000-0000-000000000099_secret_a1b2",
        };
        tokens = [
          {
            id: created.id,
            name: created.name,
            last4: created.last4,
            createdAt: created.createdAt,
            lastUsedAt: null,
          },
          ...tokens,
        ];
        if (dropCreatedTokenResponse) {
          throw new TypeError("Failed to fetch");
        }
        return jsonResponse(created, 201);
      }

      if (url.pathname.startsWith("/api/tokens/") && method === "DELETE") {
        const id = url.pathname.slice("/api/tokens/".length);
        tokens = tokens.filter((item) => item.id !== id);
        return new Response(null, { status: 204 });
      }

      return jsonResponse({ error: `unhandled ${method} ${url.pathname}` }, 404);
    });
  });

  afterEach(() => {
    dispose?.();
    dispose = undefined;
    vi.restoreAllMocks();
  });

  it("registers an account and lands in the app shell", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    dispose = render(() => <App />, host);

    await waitFor(() => {
      expect(host.querySelector("h1")?.textContent).toBe("Sign in");
    });

    const toggle = Array.from(host.querySelectorAll("button")).find((btn) =>
      btn.textContent?.includes("Need an account? Create one"),
    );
    expect(toggle).toBeDefined();
    toggle!.click();
    flush();

    expect(host.querySelector("h1")?.textContent).toBe("Create account");

    setInputValue(host.querySelector("#auth-name") as HTMLInputElement, "Dad");
    setInputValue(
      host.querySelector("#auth-email") as HTMLInputElement,
      "dad@example.com",
    );
    setInputValue(
      host.querySelector("#auth-password") as HTMLInputElement,
      "very-strong-passphrase",
    );

    host
      .querySelector("form")!
      .dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));

    await waitFor(() => {
      expect(host.querySelector("h1")?.textContent).toBe(
        "Ready for Rust Home Server",
      );
      expect(host.textContent).toContain("Signed in as Dad");
    });
  });

  it("shows inline alert when sign-in credentials are rejected", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    dispose = render(() => <App />, host);

    await waitFor(() => {
      expect(host.querySelector("h1")?.textContent).toBe("Sign in");
    });

    setInputValue(
      host.querySelector("#auth-email") as HTMLInputElement,
      "leo@example.com",
    );
    setInputValue(
      host.querySelector("#auth-password") as HTMLInputElement,
      "wrong-password",
    );

    host
      .querySelector("form")!
      .dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));

    await waitFor(() => {
      const alert = host.querySelector('[role="alert"]');
      expect(alert?.textContent).toBe("invalid email or password");
    });
  });

  it("keeps API token details collapsed until opened, supports mint/revoke, theme switch, and sign-out", async () => {
    currentUser = {
      id: "00000000-0000-0000-0000-000000000001",
      email: "leo@example.com",
      name: "Leo",
      createdAt: "2026-04-20T00:00:00Z",
    };

    const host = document.createElement("div");
    document.body.appendChild(host);
    dispose = render(() => <App />, host);

    await waitFor(() => {
      expect(host.querySelector("h1")?.textContent).toBe(
        "Ready for Rust Home Server",
      );
    });

    // Progressive disclosure: tokens are not fetched until the details panel opens.
    expect(tokenLoads).toBe(0);

    const themeSelect = host.querySelector(
      'select[aria-label="Color theme"]',
    ) as HTMLSelectElement;
    themeSelect.value = "dark";
    themeSelect.dispatchEvent(new Event("change", { bubbles: true }));
    flush();
    expect(document.documentElement.dataset.theme).toBe("dark");

    const details = host.querySelector("details") as HTMLDetailsElement;
    details.open = true;
    details.dispatchEvent(new Event("toggle"));

    await waitFor(() => {
      expect(tokenLoads).toBe(1);
    });

    setInputValue(
      host.querySelector("#new-token-name") as HTMLInputElement,
      "rust-home-server-mcp",
    );
    details
      .querySelector("form")!
      .dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));

    await waitFor(() => {
      expect(host.textContent).toContain(
        "pat_00000000-0000-0000-0000-000000000099_secret_a1b2",
      );
      expect(host.querySelector(".token-name")?.textContent).toBe(
        "rust-home-server-mcp",
      );
    });

    const revokeBtn = Array.from(host.querySelectorAll("button")).find(
      (btn) => btn.textContent === "Revoke",
    );
    revokeBtn!.click();

    await waitFor(() => {
      expect(host.querySelector(".token-name")).toBeNull();
    });

    const signOutBtn = Array.from(host.querySelectorAll("button")).find(
      (btn) => btn.textContent === "Sign out",
    );
    signOutBtn!.click();

    await waitFor(() => {
      expect(host.querySelector("h1")?.textContent).toBe("Sign in");
    });
  });

  it("handles network failures on sign-in, token load, and sign-out without corrupting UI state", async () => {
    failLoginNetwork = true;
    failTokenLoad = true;
    failLogoutStatus = true;

    const host = document.createElement("div");
    document.body.appendChild(host);
    dispose = render(() => <App />, host);

    await waitFor(() => {
      expect(host.querySelector("h1")?.textContent).toBe("Sign in");
    });

    setInputValue(
      host.querySelector("#auth-email") as HTMLInputElement,
      "leo@example.com",
    );
    setInputValue(
      host.querySelector("#auth-password") as HTMLInputElement,
      "correct-horse-battery",
    );
    host
      .querySelector("form")!
      .dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));

    await waitFor(() => {
      expect(host.querySelector('[role="alert"]')?.textContent).toContain(
        "Could not reach the server",
      );
      expect(
        (host.querySelector('button[type="submit"]') as HTMLButtonElement).disabled,
      ).toBe(false);
    });

    failLoginNetwork = false;
    host
      .querySelector("form")!
      .dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));

    await waitFor(() => {
      expect(host.querySelector("h1")?.textContent).toBe(
        "Ready for Rust Home Server",
      );
    });

    const details = host.querySelector("details") as HTMLDetailsElement;
    details.open = true;
    details.dispatchEvent(new Event("toggle"));

    await waitFor(() => {
      expect(host.textContent).toContain("database unavailable");
    });
    expect(host.textContent).not.toContain(
      "No personal access tokens minted for this account yet.",
    );

    // Recover from a committed mint whose response dropped: reopening refreshes tokens.
    failTokenLoad = false;
    details.open = false;
    details.dispatchEvent(new Event("toggle"));
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    await waitFor(() => {
      expect(host.textContent).toContain(
        "No personal access tokens minted for this account yet.",
      );
    });

    dropCreatedTokenResponse = true;
    setInputValue(
      host.querySelector("#new-token-name") as HTMLInputElement,
      "dropped-response-token",
    );
    details
      .querySelector("form")!
      .dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));
    await waitFor(() => {
      expect(host.textContent).toContain("Reopen this panel to check existing tokens");
    });

    details.open = false;
    details.dispatchEvent(new Event("toggle"));
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    await waitFor(() => {
      expect(host.querySelector(".token-name")?.textContent).toBe(
        "dropped-response-token",
      );
    });

    const signOutBtn = Array.from(host.querySelectorAll("button")).find(
      (btn) => btn.textContent === "Sign out",
    );
    signOutBtn!.click();

    await waitFor(() => {
      expect(host.textContent).toContain("session store unavailable");
      expect(host.querySelector("h1")?.textContent).toBe(
        "Ready for Rust Home Server",
      );
    });
  });

  it("preserves authenticated user if /api/app fails and ignores stale token-list responses", async () => {
    failAppBootstrap = true;
    currentUser = {
      id: "00000000-0000-0000-0000-000000000001",
      email: "leo@example.com",
      name: "Leo",
      createdAt: "2026-04-20T00:00:00Z",
    };
    const initialToken: ApiToken = {
      id: "00000000-0000-0000-0000-000000000077",
      name: "old-token",
      last4: "7777",
      createdAt: "2026-04-20T00:00:00Z",
      lastUsedAt: null,
    };
    tokens = [initialToken];

    const host = document.createElement("div");
    document.body.appendChild(host);
    dispose = render(() => <App />, host);

    await waitFor(() => {
      expect(host.querySelector("h1")?.textContent).toBe(
        "Ready for Rust Home Server",
      );
      expect(host.textContent).toContain("Signed in as Leo");
    });

    const details = host.querySelector("details") as HTMLDetailsElement;
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    await waitFor(() => {
      expect(host.querySelector(".token-name")?.textContent).toBe("old-token");
    });

    // Start a slow reopen refresh, then revoke before that stale GET resolves.
    pendingTokenLoad = () => {};
    details.open = false;
    details.dispatchEvent(new Event("toggle"));
    details.open = true;
    details.dispatchEvent(new Event("toggle"));

    const resolveStaleGet = pendingTokenLoad!;
    pendingTokenLoad = null;

    const revokeBtn = Array.from(host.querySelectorAll("button")).find(
      (btn) => btn.textContent === "Revoke",
    );
    revokeBtn!.click();
    await waitFor(() => {
      expect(host.querySelector(".token-name")).toBeNull();
    });

    resolveStaleGet(jsonResponse([initialToken]));
    await new Promise((resolve) => setTimeout(resolve, 30));
    flush();
    expect(host.querySelector(".token-name")).toBeNull();

    // Seed another old token, hold the post-mint GET open, and revoke the old
    // token before that GET settles; the newly minted token must stay visible.
    tokens = [initialToken];
    details.open = false;
    details.dispatchEvent(new Event("toggle"));
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
    await waitFor(() => {
      expect(host.querySelector(".token-name")?.textContent).toBe("old-token");
    });

    pendingTokenLoad = () => {};
    setInputValue(
      host.querySelector("#new-token-name") as HTMLInputElement,
      "fresh-token",
    );
    details
      .querySelector("form")!
      .dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true }));

    await waitFor(() => {
      const names = Array.from(host.querySelectorAll(".token-name")).map(
        (el) => el.textContent,
      );
      expect(names).toEqual(["fresh-token", "old-token"]);
    });

    const resolvePostMintGet = pendingTokenLoad!;
    pendingTokenLoad = null;

    const oldRow = Array.from(host.querySelectorAll(".token-item")).find((el) =>
      el.textContent?.includes("old-token"),
    )!;
    (oldRow.querySelector("button") as HTMLButtonElement).click();

    await waitFor(() => {
      const names = Array.from(host.querySelectorAll(".token-name")).map(
        (el) => el.textContent,
      );
      expect(names).toEqual(["fresh-token"]);
    });

    resolvePostMintGet(jsonResponse(tokens));
    await new Promise((resolve) => setTimeout(resolve, 30));
    flush();
    expect(
      Array.from(host.querySelectorAll(".token-name")).map((el) => el.textContent),
    ).toEqual(["fresh-token"]);
  });

  it("shows Cloudflare Access login link when enabled and redirects to logoutUrl on sign-out", async () => {
    cloudflareAccessEnabled = true;

    const host = document.createElement("div");
    document.body.appendChild(host);
    dispose = render(() => <App />, host);

    await waitFor(() => {
      const link = host.querySelector(
        'a[href="/cdn-cgi/access/login"]',
      ) as HTMLAnchorElement | null;
      expect(link?.textContent).toBe("Sign in with Cloudflare Access");
    });

    dispose();
    host.remove();

    currentUser = {
      id: "00000000-0000-0000-0000-000000000003",
      email: "cf@example.com",
      name: "cf",
      createdAt: "2026-04-20T00:00:00Z",
    };
    logoutUrl = "/cdn-cgi/access/logout";
    const assignSpy = vi.fn();
    const originalLocation = window.location;
    Object.defineProperty(window, "location", {
      configurable: true,
      value: { ...originalLocation, assign: assignSpy },
    });

    try {
      const host2 = document.createElement("div");
      document.body.appendChild(host2);
      dispose = render(() => <App />, host2);

      await waitFor(() => {
        expect(host2.textContent).toContain("Signed in as cf");
      });

      const signOutBtn = Array.from(host2.querySelectorAll("button")).find(
        (btn) => btn.textContent === "Sign out",
      );
      signOutBtn!.click();

      await waitFor(() => {
        expect(assignSpy).toHaveBeenCalledWith("/cdn-cgi/access/logout");
      });
    } finally {
      Object.defineProperty(window, "location", {
        configurable: true,
        value: originalLocation,
      });
    }
  });
});
