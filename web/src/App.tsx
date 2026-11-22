import { Show } from "solid-js";
import { AppShell } from "./components/AppShell";
import { AuthScreen } from "./components/AuthScreen";
import { AuthProvider, useAuth } from "./lib/auth";

function RootContent() {
  const auth = useAuth();

  return (
    <Show
      when={auth.ready()}
      fallback={<div class="status-center">Loading workspace...</div>}
    >
      <Show when={auth.user()} fallback={<AuthScreen />}>
        {(signedInUser) => <AppShell user={signedInUser()} />}
      </Show>
    </Show>
  );
}

export function App() {
  return (
    <AuthProvider>
      <RootContent />
    </AuthProvider>
  );
}
