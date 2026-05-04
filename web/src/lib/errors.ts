export function errorMessage(error: unknown, fallback = "request failed"): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return fallback;
}
