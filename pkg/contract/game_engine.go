// SPDX-License-Identifier: AGPL-3.0-only

package contract

import "context"

// GameEngine is the boundary for native engines and delegated upstream
// backends. Commands must be idempotent and state-aware because UI retries,
// runtime recovery, and external backend restarts are expected.
type GameEngine interface {
	Describe(context.Context) (GameEngineDescriptor, error)
	ResolveProfile(context.Context, ProfileID) (ProfileSummary, error)
	Status(context.Context, ProfileID) (RuntimeStatus, error)
	Start(context.Context, RuntimeCommand) (CommandResult, error)
	Stop(context.Context, RuntimeCommand) (CommandResult, error)
	Restart(context.Context, RuntimeCommand) (CommandResult, error)
	Refresh(context.Context, RuntimeCommand) (CommandResult, error)
	SubmitTask(context.Context, TaskRequest) (TaskRunSummary, error)
	Scheduler(context.Context, ProfileID) (SchedulerSummary, error)
	RecentLogs(context.Context, RecentQuery) ([]LogEvent, error)
	ResourceHistory(context.Context, ResourceHistoryQuery) ([]ResourceHistoryPoint, error)
	RecentAcquisitions(context.Context, AcquisitionQuery) ([]AcquisitionCapture, error)
}

type GameEngineDescriptor struct {
	ID                   string              `json:"id"`
	Kind                 EngineKind          `json:"kind"`
	Game                 GameKey             `json:"game"`
	SupportedServers     []ServerKey         `json:"supportedServers"`
	SupportedResolutions []Resolution        `json:"supportedResolutions"`
	Capabilities         []RuntimeCapability `json:"capabilities,omitempty"`
	Version              string              `json:"version,omitempty"`
}

type RuntimeCommand struct {
	ProfileID ProfileID         `json:"profileId"`
	RequestID string            `json:"requestId"`
	Reason    string            `json:"reason,omitempty"`
	Options   map[string]string `json:"options,omitempty"`
}

type CommandResult struct {
	RequestID string        `json:"requestId"`
	State     RuntimeState  `json:"state"`
	Accepted  bool          `json:"accepted"`
	Message   string        `json:"message,omitempty"`
	Error     *RuntimeError `json:"error,omitempty"`
}

type TaskRequest struct {
	ProfileID ProfileID         `json:"profileId"`
	RequestID string            `json:"requestId"`
	TaskID    TaskID            `json:"taskId"`
	FlowID    string            `json:"flowId"`
	Options   map[string]string `json:"options,omitempty"`
}

type TaskRunSummary struct {
	TaskRunID TaskRunID     `json:"taskRunId"`
	TaskID    TaskID        `json:"taskId"`
	ProfileID ProfileID     `json:"profileId"`
	State     RuntimeState  `json:"state"`
	StartedAt string        `json:"startedAt"`
	EndedAt   string        `json:"endedAt,omitempty"`
	LastError *RuntimeError `json:"lastError,omitempty"`
}

type RecentQuery struct {
	ProfileID ProfileID `json:"profileId,omitempty"`
	Limit     int       `json:"limit,omitempty"`
}

type ResourceHistoryQuery struct {
	ProfileID ProfileID   `json:"profileId,omitempty"`
	Key       ResourceKey `json:"key,omitempty"`
	Limit     int         `json:"limit,omitempty"`
}

type AcquisitionQuery struct {
	ProfileID ProfileID `json:"profileId,omitempty"`
	TaskRunID TaskRunID `json:"taskRunId,omitempty"`
	Limit     int       `json:"limit,omitempty"`
}
