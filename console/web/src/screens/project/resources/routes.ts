/** Where each authoritative Resources view lives in the browser. */
export const RESOURCE_ROUTES = {
  Resources: { to: ".", end: true },
  Observation: { to: "observation" },
} as const;
