/* Shapes returned by the Workgraph API (subset used by the UI). */

export type TaskStatus =
  | "backlog"
  | "ready"
  | "blocked"
  | "claimed"
  | "running"
  | "awaiting_verification"
  | "awaiting_review"
  | "accepted"
  | "failed"
  | "cancelled";

export const TASK_STATUSES: TaskStatus[] = [
  "backlog",
  "ready",
  "blocked",
  "claimed",
  "running",
  "awaiting_verification",
  "awaiting_review",
  "accepted",
  "failed",
  "cancelled",
];
export const PRIORITIES = ["critical", "high", "medium", "low"] as const;
export type Priority = (typeof PRIORITIES)[number];
export const CATEGORIES = [
  "bug_fix", "feature", "refactor", "maintenance", "infrastructure", "performance", "security", "testing",
  "documentation", "research", "architecture", "other",
] as const;

export interface Meta {
  version: string;
  default_workspace: string | null;
  execution_enabled: boolean;
  auto_schedule: boolean;
  agents: string[];
  llm: Record<string, { provider: string; model: string }>;
}

export interface Workspace {
  id: string;
  slug: string;
  name: string;
  description: string;
  key_prefix: string;
  created_at: string;
}

export interface WorkspaceSummary {
  workspace: Workspace;
  projects: number;
  repositories: number;
  tasks_by_status: Record<string, number>;
  ready_tasks: number;
  running_attempts: number;
  queued_jobs: number;
  dead_jobs: number;
}

export interface Project {
  id: string;
  slug: string;
  name: string;
  description: string;
  status: string;
  repository_ids: string[];
}

export interface Checkout {
  id: string;
  path: string;
  kind: string;
  is_primary: boolean;
  available: boolean;
  last_seen_at: string | null;
}

export interface Repository {
  id: string;
  name: string;
  description: string;
  identity_fingerprint: string | null;
  default_branch: string | null;
  tracked_branches: string[];
  access_mode: string;
  health_status: string;
  health_checked_at: string | null;
  is_shallow: boolean;
  last_scanned_at: string | null;
  archived_at: string | null;
  remotes: { name: string; url: string }[];
  checkouts: Checkout[];
  project_ids: string[];
  depends_on: string[];
}

export interface RepositoryStatus {
  available: boolean;
  checkout_path: string | null;
  branch: string | null;
  head: string | null;
  staged: string[];
  unstaged: string[];
  untracked: string[];
  ahead: number | null;
  behind: number | null;
  error: string | null;
}

export interface Task {
  id: string;
  key: string;
  number: number;
  title: string;
  description: string;
  acceptance_criteria: string[];
  status: TaskStatus;
  priority: Priority;
  category: string | null;
  tags: string[];
  project_ids: string[];
  repository_ids: string[];
  parent_id: string | null;
  child_ids: string[];
  depends_on: string[];
  blocks: string[];
  assignee: string | null;
  assigned_agent: string | null;
  claimed_by: string | null;
  auto_execute: boolean;
  version: number;
  created_by: string;
  created_at: string;
  updated_at: string;
  started_at: string | null;
  completed_at: string | null;
  accepted_at: string | null;
  archived_at: string | null;
}

export interface Comment {
  id: string;
  attempt_id: string | null;
  author: string;
  kind: string;
  body: string;
  created_at: string;
}

export interface TaskDetail extends Task {
  relations: { id: string; source_task_id: string; target_task_id: string; relation_type: string }[];
  comments: Comment[];
  rollup: { total_descendants: number; by_status: Record<string, number>; accepted: number; progress: number } | null;
  dependency_state: { satisfied: boolean; unsatisfied: { id: string; key: string; title: string; status: string }[] } | null;
}

export interface Page<T> {
  items: T[];
  total: number;
  limit: number;
  offset: number;
  next_offset: number | null;
}

export interface EventOut {
  seq: number;
  id: string;
  event_type: string;
  entity_type: string;
  entity_id: string;
  actor: string;
  actor_kind: string;
  occurred_at: string;
  attempt_id: string | null;
  data: Record<string, unknown>;
}

export type StackStatus = "open" | "integrating" | "verifying" | "verified" | "landed" | "abandoned";

export type StackItemState = "pending" | "ready" | "merged" | "conflict" | "ejected" | "verified";

export interface StackRepository {
  repository_id: string;
  repository_name: string;
  base_sha: string;
  integration_branch: string;
  integration_tip: string | null;
}

export interface StackItem {
  id: string;
  stack_id: string;
  task_id: string;
  task_key: string;
  task_title: string;
  state: StackItemState;
  merge_order: number | null;
  detail: string | null;
  created_at: string;
  updated_at: string;
}

export interface Stack {
  id: string;
  workspace_id: string;
  parent_task_id: string;
  parent_key: string;
  parent_title: string;
  status: StackStatus;
  repositories: StackRepository[];
  items: StackItem[];
  created_at: string;
  updated_at: string;
}

export interface StackIntegrationJobs {
  stack: string;
  jobs: { repository: string; job_id: string }[];
}

export interface StackVerificationJob {
  stack: string;
  job_id: string;
}

export interface Attempt {
  id: string;
  task_id: string;
  number: number;
  status: string;
  provider: string;
  adapter: string;
  agent_id: string | null;
  model: string | null;
  session_id: string | null;
  parent_attempt_id: string | null;
  retry_of_attempt_id: string | null;
  rerun_of_attempt_id: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
  queue_wait_ms: number | null;
  duration_ms: number | null;
  exit_reason: string | null;
  exit_code: number | null;
  error: string | null;
  summary: string | null;
  usage: Record<string, number>;
  usage_source: string | null;
  cost_usd: number | null;
  telemetry: Record<string, unknown>;
  modified_repository_ids: string[];
}

export interface RunOut {
  attempt: Attempt;
  job_id: string | null;
  job_status: string | null;
  recent_events: EventOut[];
}

export interface CommitLink {
  source: string;
  confidence: number;
  evidence: Record<string, unknown>;
  attempt_id: string | null;
}

export interface TaskCommit {
  commit_id: string;
  sha: string;
  repository_id: string;
  repository: string;
  subject: string;
  author: string;
  committed_at: string | null;
  insertions: number;
  deletions: number;
  files_changed: number;
  is_merge: boolean;
  reachable: boolean;
  links: CommitLink[];
  confidence: number;
}

export interface TaskChanges {
  commits: TaskCommit[];
  cumulative_churn: Record<string, unknown> & {
    commits: number;
    insertions: number;
    deletions: number;
    files_touched: number;
    repeated_file_edits: number;
    functions_affected: number;
  };
  final_change_surface: {
    repository_id: string;
    base_sha: string;
    final_sha: string;
    files_changed: number;
    insertions: number;
    deletions: number;
    renames: number;
    binary_files: number;
    functions_affected: number;
    types_affected: number;
  }[];
  definitions: Record<string, string>;
}

export interface CheckResult {
  id: string;
  name: string;
  kind: string;
  required: boolean;
  status: string;
  exit_code: number | null;
  duration_ms: number | null;
  timed_out: boolean;
  output_excerpt: string;
  failure_signatures: string[];
}

export interface VerificationRun {
  id: string;
  task_id: string;
  attempt_id: string | null;
  status: string;
  required_passed: boolean;
  triggered_by: string;
  created_at: string;
  manifest: { repository: string; sha: string | null; access: string }[];
  checks: CheckResult[];
}

export interface Classification {
  id: string;
  source: string;
  status: string;
  provider: string | null;
  model: string | null;
  prompt_version: string | null;
  taxonomy_version: string;
  category: string | null;
  subtype: string | null;
  domain: string | null;
  complexity: string | null;
  tags: string[];
  confidence: number | null;
  rationale: string | null;
  validation_status: string;
  error: string | null;
  is_current: boolean;
  created_at: string;
}

export interface TaskClassifications {
  task_key: string;
  current: Classification | null;
  history: Classification[];
  disagreement: boolean;
  note: string;
}

export interface GraphNode {
  id: string;
  key: string;
  title: string;
  status: TaskStatus;
  priority: string;
  category: string | null;
  parent_id: string | null;
  project_ids: string[];
  repository_ids: string[];
  tags: string[];
  blocked: boolean;
}

export interface GraphEdge {
  source: string;
  target: string;
  kind: "depends_on" | "parent" | "relation";
  relation_type: string | null;
}

export interface Job {
  id: string;
  kind: string;
  status: string;
  attempts: number;
  max_attempts: number;
  run_at: string;
  last_error: string | null;
  error_kind: string | null;
  task_id: string | null;
  attempt_id: string | null;
  created_at: string;
  payload: Record<string, unknown>;
}

export interface Worker {
  id: string;
  hostname: string;
  pid: number;
  status: string;
  concurrency: number;
  current_jobs: string[];
  jobs_completed: number;
  jobs_failed: number;
  heartbeat_at: string;
}

export interface Rule {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  trigger: Record<string, unknown>;
  conditions: Record<string, unknown>;
  actions: Record<string, unknown>[];
  retry_max_attempts: number;
  max_depth: number;
  created_by: string;
  version: number;
  next_run_at: string | null;
}

export interface AutomationRun {
  id: string;
  rule_id: string;
  trigger_type: string;
  trigger_key: string;
  depth: number;
  dry_run: boolean;
  status: string;
  matched: boolean;
  reason: string;
  condition_trace: { field: string; op: string; expected: unknown; actual: unknown; result: boolean }[];
  action_results: Record<string, unknown>[];
  error: string | null;
  created_at: string;
}

export interface MetricDefinition {
  id: string;
  family: string;
  title: string;
  definition: string;
  unit: string;
  statistic: string;
  dimensions: string[];
  caveats: string[];
}

export type Row = Record<string, string | number | null>;

export interface AnalyticsResult {
  metric: MetricDefinition;
  rows: Row[];
  sample_size: number;
  notes: string[];
  provenance: { sql: string };
}

/** `GET /ws/{ws}/analytics/bottlenecks`: where subagent wall time went (see analytics/agent_time.py). */
export interface AgentTimeClass {
  class: string;
  kind: "tool" | "model" | "waiting";
  seconds: number;
  calls: number;
  share: number | null;
}

export interface SlowCall {
  class: string;
  seconds: number;
  tool: string | null;
  command: string | null;
  at: string | null;
  description: string;
  model: string | null;
  task: string | null;
  concurrent_runs: number | null;
}

export interface TaskWallTime {
  key: string;
  runs: number;
  span_seconds: number;
  accounted_seconds: number;
  waiting_seconds: number;
  polling_share: number | null;
  top_classes: { class: string; seconds: number }[];
}

export interface Bottlenecks {
  totals: {
    runs: number;
    span_seconds: number;
    accounted_seconds: number;
    waiting_share: number | null;
    polling_share: number | null;
    model_share: number | null;
  };
  classes: AgentTimeClass[];
  slowest_calls: SlowCall[];
  concurrency: { peak_during_window: number; median_at_slowest_calls: number | null; scope: string };
  by_task: TaskWallTime[];
  notes: string[];
}
