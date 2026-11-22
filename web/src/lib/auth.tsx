import type { JSX } from "@solidjs/web";
import {
  createContext,
  createSignal,
  onSettled,
  useContext,
  type Accessor,
} from "solid-js";
import { api, type LoginRequest, type RegisterRequest, type User } from "./api/client";

export interface AuthStore {
  ready: Accessor<boolean>;
  user: Accessor<User | null>;
  registrationOpen: Accessor<boolean>;
  login: (input: LoginRequest) => Promise<{ error?: string }>;
  register: (input: RegisterRequest) => Promise<{ error?: string }>;
  logout: () => Promise<{ error?: string }>;
}

const AuthContext = createContext<AuthStore>();

export function AuthProvider(props: { children: JSX.Element }) {
  const [ready, setReady] = createSignal(false);
  const [user, setUser] = createSignal<User | null>(null);
  const [registrationOpen, setRegistrationOpen] = createSignal(false);

  async function refreshBootstrap() {
    const [appRes, meRes] = await Promise.allSettled([
      api.GET("/api/app"),
      api.GET("/api/auth/me"),
    ]);
    if (appRes.status === "fulfilled" && appRes.value.data) {
      setRegistrationOpen(appRes.value.data.registrationOpen);
    }
    setUser(meRes.status === "fulfilled" ? (meRes.value.data ?? null) : null);
    setReady(true);
  }

  onSettled(() => {
    void refreshBootstrap();
  });

  async function login(input: LoginRequest): Promise<{ error?: string }> {
    try {
      const { data, error } = await api.POST("/api/auth/login", { body: input });
      if (error || !data) {
        return { error: error?.error ?? "Unable to sign in." };
      }
      setUser(data);
      return {};
    } catch {
      return { error: "Could not reach the server to sign in." };
    }
  }

  async function register(input: RegisterRequest): Promise<{ error?: string }> {
    let created: User;
    try {
      const { data, error } = await api.POST("/api/auth/register", { body: input });
      if (error || !data) {
        return { error: error?.error ?? "Unable to create account." };
      }
      created = data;
    } catch {
      return { error: "Could not reach the server to create your account." };
    }
    setUser(created);
    try {
      const appRes = await api.GET("/api/app");
      if (appRes.data) {
        setRegistrationOpen(appRes.data.registrationOpen);
      }
    } catch {
      // Optional post-register bootstrap refresh; keep the signed-in session active.
    }
    return {};
  }

  async function logout(): Promise<{ error?: string }> {
    try {
      const { error, response } = await api.POST("/api/auth/logout");
      if (error || !response.ok) {
        return { error: error?.error ?? "Unable to sign out." };
      }
    } catch {
      return { error: "Could not reach the server to sign out." };
    }
    setUser(null);
    try {
      const appRes = await api.GET("/api/app");
      if (appRes.data) {
        setRegistrationOpen(appRes.data.registrationOpen);
      }
    } catch {
      // Optional post-logout bootstrap refresh.
    }
    return {};
  }

  const store: AuthStore = {
    ready,
    user,
    registrationOpen,
    login,
    register,
    logout,
  };

  return <AuthContext value={store}>{props.children}</AuthContext>;
}

export function useAuth(): AuthStore {
  const store = useContext(AuthContext);
  if (!store) {
    throw new Error("useAuth must be used inside <AuthProvider>");
  }
  return store;
}
