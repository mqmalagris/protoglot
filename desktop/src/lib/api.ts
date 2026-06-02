// Typed wrappers over the Tauri commands defined in src-tauri/src/main.rs.
// These mirror protoglot-core's serialized types.
import { invoke } from "@tauri-apps/api/core";

export interface RequestInfo {
  name: string;
  kind: string;
  path: string;
}

export interface AssertionOutcome {
  description: string;
  passed: boolean;
  message?: string;
}

export interface ResponseSummary {
  status: number;
  content_type?: string;
  size_bytes: number;
}

export type ExecStatus = "ok" | "failed" | "error";

export interface ExecutionResult {
  request_name: string;
  protocol: string;
  status: ExecStatus;
  duration_ms: number;
  response?: ResponseSummary;
  assertions: AssertionOutcome[];
  error?: string;
}

export const listRequests = (path: string) =>
  invoke<RequestInfo[]>("list_requests", { path });

export const readRequest = (path: string) =>
  invoke<string>("read_request", { path });

export const runCollection = (path: string, env?: string) =>
  invoke<ExecutionResult[]>("run_collection", { path, env: env ?? null });
