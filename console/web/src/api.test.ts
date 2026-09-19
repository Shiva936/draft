// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, ConsoleApiError, establishSession } from "./api";

function response(ok: boolean, status: number, value: unknown) {
  return {
    ok,
    status,
    headers: { get: (name: string) => name === "content-type" ? "application/json" : null },
    json: async () => value,
    text: async () => JSON.stringify(value),
  } as Response;
}

describe("Console browser transport", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    history.replaceState(null, "", "/");
  });

  it("posts the fragment bootstrap once and removes it from browser history", async () => {
    history.replaceState(null, "", "/#bootstrap=one-time-secret");
    const fetch = vi.fn().mockResolvedValue(response(true, 200, {
      schema_version: 1,
      data: {
        schema_version: 1,
        authenticated: true,
        csrf_token: "csrf",
        preselected_workspace_id: "ws_a",
      },
    }));
    vi.stubGlobal("fetch", fetch);

    const session = await establishSession();

    expect(session.authenticated).toBe(true);
    expect(location.hash).toBe("");
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fetch).toHaveBeenCalledWith("/api/v1/bootstrap", expect.objectContaining({
      method: "POST",
      credentials: "same-origin",
      body: JSON.stringify({ schema_version: 1, secret: "one-time-secret" }),
    }));
  });

  it("surfaces structured gateway failures", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(response(false, 403, {
      schema_version: 1,
      error: { code: "INVALID_ORIGIN", message: "origin rejected" },
    })));

    await expect(api("/api/v1/projects")).rejects.toEqual(
      expect.objectContaining<Partial<ConsoleApiError>>({
        status: 403,
        code: "INVALID_ORIGIN",
        message: "origin rejected",
      }),
    );
  });
});
