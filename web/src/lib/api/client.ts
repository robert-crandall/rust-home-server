import createClient from "openapi-fetch";
import type { components, paths } from "./schema";

const baseUrl = typeof window !== "undefined" ? window.location.origin : "";

/**
 * Shared typed API client generated from docs/openapi.json.
 * Same-origin session cookies accompany browser requests automatically.
 */
export const api = createClient<paths>({
  baseUrl,
  fetch: (request) => globalThis.fetch(request),
});

export type User = components["schemas"]["User"];
export type AppInfo = components["schemas"]["AppInfo"];
export type ApiToken = components["schemas"]["ApiToken"];
export type CreatedApiToken = components["schemas"]["CreatedApiToken"];
export type LoginRequest = components["schemas"]["LoginRequest"];
export type RegisterRequest = components["schemas"]["RegisterRequest"];
