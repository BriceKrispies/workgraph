//! The domain enumerations of contracts section 10, serialised by their wire strings.

string_enum! {
    pub enum AccessMode {
        ReadOnly = "read_only",
        ReadWrite = "read_write",
    }
}

string_enum! {
    pub enum ActorKind {
        Human = "human",
        Agent = "agent",
        System = "system",
        Automation = "automation",
    }
}

string_enum! {
    pub enum AttemptStatus {
        Queued = "queued",
        Preparing = "preparing",
        Running = "running",
        Verifying = "verifying",
        Succeeded = "succeeded",
        Failed = "failed",
        Cancelled = "cancelled",
        TimedOut = "timed_out",
        Abandoned = "abandoned",
    }
}

string_enum! {
    pub enum AttributionSource {
        Explicit = "explicit",
        ExecutionContext = "execution_context",
        Worktree = "worktree",
        Trailer = "trailer",
        MessageInference = "message_inference",
        RewriteReconciliation = "rewrite_reconciliation",
    }
}

string_enum! {
    pub enum ChangeSetItemStatus {
        Pending = "pending",
        Committed = "committed",
        Verified = "verified",
        Failed = "failed",
    }
}

string_enum! {
    pub enum ChangeSetStatus {
        Open = "open",
        PartiallyApplied = "partially_applied",
        Applied = "applied",
        PartiallyVerified = "partially_verified",
        Verified = "verified",
        Failed = "failed",
        Abandoned = "abandoned",
    }
}

string_enum! {
    pub enum CheckKind {
        Test = "test",
        Lint = "lint",
        Typecheck = "typecheck",
        Build = "build",
        Integration = "integration",
        Custom = "custom",
    }
}

string_enum! {
    pub enum ClassificationStatus {
        Pending = "pending",
        Succeeded = "succeeded",
        Failed = "failed",
    }
}

string_enum! {
    pub enum CommentKind {
        Comment = "comment",
        Progress = "progress",
        Result = "result",
    }
}

string_enum! {
    pub enum ExperimentStatus {
        Draft = "draft",
        Running = "running",
        Closed = "closed",
    }
}

string_enum! {
    pub enum FailureCause {
        TooBig = "too_big",
        MissingContext = "missing_context",
        AmbiguousSpec = "ambiguous_spec",
        Coupling = "coupling",
        Gaming = "gaming",
        GateGap = "gate_gap",
        Harness = "harness",
        Budget = "budget",
        WrongModel = "wrong_model",
        Other = "other",
    }
}

string_enum! {
    pub enum FailureCauseSource {
        Human = "human",
        Agent = "agent",
        Mechanical = "mechanical",
    }
}

string_enum! {
    pub enum JobStatus {
        Queued = "queued",
        Running = "running",
        Succeeded = "succeeded",
        Failed = "failed",
        Dead = "dead",
        Cancelled = "cancelled",
    }
}

string_enum! {
    pub enum RelationType {
        RelatesTo = "relates_to",
        Duplicates = "duplicates",
        FollowsUp = "follows_up",
        RegressionOf = "regression_of",
        SimilarTo = "similar_to",
    }
}

string_enum! {
    pub enum StackItemState {
        Pending = "pending",
        Ready = "ready",
        Merged = "merged",
        Conflict = "conflict",
        Ejected = "ejected",
        Verified = "verified",
    }
}

string_enum! {
    pub enum StackStatus {
        Open = "open",
        Integrating = "integrating",
        Verifying = "verifying",
        Verified = "verified",
        Landed = "landed",
        Abandoned = "abandoned",
    }
}

string_enum! {
    pub enum TaskPriority {
        Critical = "critical",
        High = "high",
        Medium = "medium",
        Low = "low",
    }
}

string_enum! {
    pub enum TaskStatus {
        Backlog = "backlog",
        Ready = "ready",
        Blocked = "blocked",
        Claimed = "claimed",
        Running = "running",
        AwaitingVerification = "awaiting_verification",
        AwaitingReview = "awaiting_review",
        Accepted = "accepted",
        Failed = "failed",
        Cancelled = "cancelled",
    }
}

string_enum! {
    pub enum TrialOutcome {
        Passed = "pass",
        Failed = "fail",
    }
}

string_enum! {
    pub enum TrialPhase {
        Staircase = "staircase",
        Refine = "refine",
        Control = "control",
        Repeat = "repeat",
        Live = "live",
    }
}

string_enum! {
    pub enum TrialStatus {
        Planned = "planned",
        Running = "running",
        Graded = "graded",
        Invalid = "invalid",
    }
}

string_enum! {
    pub enum VerificationStatus {
        Pending = "pending",
        Running = "running",
        Passed = "passed",
        Failed = "failed",
        Error = "error",
        Skipped = "skipped",
    }
}

string_enum! {
    pub enum WorktreeStatus {
        Preparing = "preparing",
        Active = "active",
        Released = "released",
        Removed = "removed",
        Orphaned = "orphaned",
    }
}
