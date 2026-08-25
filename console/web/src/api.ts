import { CONTRACT_VERSIONS } from "./contracts";
import type { ApiFailure, ApiResponse, ServiceJob, Session } from "./contracts";

let csrfToken = "";

export class ConsoleApiError extends Error {
  constructor(readonly status: number, readonly code: string, message: string, readonly details?: unknown) {
    super(message);
  }
}

export async function establishSession(): Promise<Session> {
  const fragment = new URLSearchParams(window.location.hash.slice(1));
  const bootstrap = fragment.get("bootstrap");
  const response = bootstrap
    ? await fetch("/api/v1/bootstrap", {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ schema_version: CONTRACT_VERSIONS.mutationRequest, secret: bootstrap }),
      })
    : await fetch("/api/v1/session", { credentials: "same-origin" });
  if (bootstrap) history.replaceState(history.state, "", `${location.pathname}${location.search}`);
  const session = await decode<Session>(response);
  csrfToken = session.csrf_token;
  return session;
}

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  return decode<T>(await fetch(path, { ...init, credentials: "same-origin" }));
}

export function mutate<T>(path: string, body: unknown = {}): Promise<T> {
  if (body === null || typeof body !== "object" || Array.isArray(body)) {
    throw new TypeError("Draft request bodies must be JSON objects");
  }
  return api<T>(path, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-draft-csrf": csrfToken,
      "x-draft-operation-id": `op_${crypto.randomUUID().replaceAll("-", "")}`,
    },
    body: JSON.stringify({ ...body, schema_version: CONTRACT_VERSIONS.mutationRequest }),
  });
}

export function isServiceJob(value: unknown): value is ServiceJob {
  const candidate = value as Partial<ServiceJob> | null;
  return candidate?.schema_version === CONTRACT_VERSIONS.serviceJob && typeof candidate.id === "string";
}

/** Wait for a durable daemon job while preserving its canonical server state. */
export async function settleJob<T>(value: T, onProgress?: (job: ServiceJob) => void): Promise<T | ServiceJob> {
  if (!isServiceJob(value)) return value;
  let job: ServiceJob = value;
  onProgress?.(job);
  while (job.status === "queued" || job.status === "running") {
    await new Promise((resolve) => window.setTimeout(resolve, 200));
    job = await api<ServiceJob>(`/api/v1/jobs/${encodeURIComponent(job.id)}`);
    onProgress?.(job);
  }
  if (job.status === "failed" || job.status === "cancelled") {
    throw new ConsoleApiError(
      job.status === "cancelled" ? 409 : 500,
      job.status === "cancelled" ? "JOB_CANCELLED" : "JOB_FAILED",
      job.error ?? `${job.kind} ${job.status}`,
      job,
    );
  }
  return job;
}

export function cancelJob(jobId: string): Promise<ServiceJob> {
  return mutate(`/api/v1/jobs/${encodeURIComponent(jobId)}/cancel`);
}

async function decode<T>(response: Response): Promise<T> {
  const value = response.headers.get("content-type")?.includes("application/json")
    ? ((await response.json()) as ApiResponse<T> | ApiFailure)
    : ((await response.text()) as T);
  if (typeof value === "object" && value !== null && "schema_version" in value && value.schema_version !== CONTRACT_VERSIONS.apiEnvelope && value.schema_version !== CONTRACT_VERSIONS.apiFailure) {
    throw new ConsoleApiError(response.status, "UNSUPPORTED_SCHEMA", "Console response schema is unsupported", value);
  }
  if (!response.ok) {
    const failure = value as ApiFailure;
    throw new ConsoleApiError(response.status, failure.error?.code ?? "HTTP_ERROR", failure.error?.message ?? String(value), failure.error?.details);
  }
  const envelope = value as ApiResponse<T>;
  if (envelope.schema_version !== CONTRACT_VERSIONS.apiEnvelope || !("data" in envelope)) {
    throw new ConsoleApiError(response.status, "VALIDATION_ERROR", "Console response envelope is invalid", value);
  }
  return envelope.data;
}
