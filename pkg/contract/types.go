// SPDX-License-Identifier: AGPL-3.0-only

// Package contract defines the P0a boundary between the ActingCommand UI,
// decision/data core, and future execution layer.
package contract

import "time"

type GameKey string

const (
	GameAzur GameKey = "Azur"
	GameArk  GameKey = "Ark"
	GameBA   GameKey = "BA"
)

type ServerKey string

const (
	ServerAlasCN       ServerKey = "alas.cn"
	ServerAlasEN       ServerKey = "alas.en"
	ServerAlasJP       ServerKey = "alas.jp"
	ServerAlasTW       ServerKey = "alas.tw"
	ServerBAASJP       ServerKey = "baas.jp"
	ServerBAASCN       ServerKey = "baas.cn"
	ServerBAASGlobalEN ServerKey = "baas.global_en"
	ServerBAASKO       ServerKey = "baas.ko"
	ServerBAASZHTW     ServerKey = "baas.zh_tw"
	ServerMAABilibili  ServerKey = "maa.bilibili"
	ServerMAAOfficial  ServerKey = "maa.official"
	ServerMAATxwy      ServerKey = "maa.txwy"
	ServerMAAYostarEN  ServerKey = "maa.yostar_en"
	ServerMAAYostarJP  ServerKey = "maa.yostar_jp"
	ServerMAAYostarKR  ServerKey = "maa.yostar_kr"
)

type EngineKind string

const (
	EngineNative    EngineKind = "native"
	EngineDelegated EngineKind = "delegated"
)

type RuntimeState string

const (
	RuntimeUnknown  RuntimeState = "unknown"
	RuntimeStopped  RuntimeState = "stopped"
	RuntimeStarting RuntimeState = "starting"
	RuntimeRunning  RuntimeState = "running"
	RuntimeStopping RuntimeState = "stopping"
	RuntimeDegraded RuntimeState = "degraded"
	RuntimeFatal    RuntimeState = "fatal"
)

type Severity string

const (
	SeverityInfo     Severity = "info"
	SeverityWarning  Severity = "warning"
	SeverityError    Severity = "error"
	SeverityFatal    Severity = "fatal"
	SeverityDegraded Severity = "degraded"
)

type ProfileID string
type TaskID string
type TaskRunID string
type ResourceKey string

type Resolution struct {
	Width  int     `json:"width"`
	Height int     `json:"height"`
	Scale  float64 `json:"scale,omitempty"`
	DPI    int     `json:"dpi,omitempty"`
}

type RuntimeError struct {
	Severity          Severity          `json:"severity"`
	Code              string            `json:"code"`
	Message           string            `json:"message"`
	Module            string            `json:"module"`
	OriginalError     string            `json:"originalError,omitempty"`
	FallbackPath      string            `json:"fallbackPath,omitempty"`
	UserVisibleImpact string            `json:"userVisibleImpact,omitempty"`
	Context           map[string]string `json:"context,omitempty"`
	OccurredAt        time.Time         `json:"occurredAt"`
}

type LogEvent struct {
	Timestamp time.Time         `json:"timestamp"`
	Level     Severity          `json:"level"`
	Source    string            `json:"source"`
	Message   string            `json:"message"`
	Context   map[string]string `json:"context,omitempty"`
}

type SchedulerSummary struct {
	Alive        bool         `json:"alive"`
	CurrentTask  string       `json:"currentTask,omitempty"`
	NextTask     string       `json:"nextTask,omitempty"`
	NextRunAt    *time.Time   `json:"nextRunAt,omitempty"`
	PendingCount int          `json:"pendingCount"`
	WaitingCount int          `json:"waitingCount"`
	LastSeverity Severity     `json:"lastSeverity"`
	State        RuntimeState `json:"state"`
}

type ProfileSummary struct {
	ID                 ProfileID                `json:"id"`
	Name               string                   `json:"name"`
	Game               GameKey                  `json:"game"`
	Server             ServerKey                `json:"server"`
	Locale             string                   `json:"locale,omitempty"`
	Resolution         Resolution               `json:"resolution"`
	RuntimeState       RuntimeState             `json:"runtimeState"`
	Scheduler          SchedulerSummary         `json:"scheduler"`
	ResourceSnapshot   map[ResourceKey]Resource `json:"resourceSnapshot,omitempty"`
	ResourceHistory    []ResourceHistoryPoint   `json:"resourceHistory,omitempty"`
	RecentAcquisitions []AcquisitionCapture     `json:"recentAcquisitions,omitempty"`
	RecentLogs         []LogEvent               `json:"recentLogs,omitempty"`
}

type Resource struct {
	Key        ResourceKey `json:"key"`
	Value      string      `json:"value"`
	ObservedAt time.Time   `json:"observedAt"`
	Source     string      `json:"source"`
}

type ResourceHistoryPoint struct {
	Timestamp time.Time   `json:"timestamp"`
	ProfileID ProfileID   `json:"profileId"`
	Game      GameKey     `json:"game"`
	Server    ServerKey   `json:"server"`
	Key       ResourceKey `json:"key"`
	Value     string      `json:"value"`
	Source    string      `json:"source"`
}

type AcquisitionCapture struct {
	ID               string     `json:"id"`
	ProfileID        ProfileID  `json:"profileId"`
	Game             GameKey    `json:"game"`
	Server           ServerKey  `json:"server"`
	Locale           string     `json:"locale,omitempty"`
	Resolution       Resolution `json:"resolution"`
	TaskID           TaskID     `json:"taskId"`
	TaskRunID        TaskRunID  `json:"taskRunId"`
	CapturedAt       time.Time  `json:"capturedAt"`
	ImageRef         string     `json:"imageRef"`
	ImageHash        string     `json:"imageHash,omitempty"`
	SourceTrigger    string     `json:"sourceTrigger"`
	RecognitionState string     `json:"recognitionState"`
	Labels           []string   `json:"labels,omitempty"`
	RetentionClass   string     `json:"retentionClass,omitempty"`
}

type RuntimeStatus struct {
	State        RuntimeState        `json:"state"`
	StartedAt    *time.Time          `json:"startedAt,omitempty"`
	StateDir     string              `json:"stateDir"`
	Version      string              `json:"version"`
	Scheduler    SchedulerSummary    `json:"scheduler"`
	LastError    *RuntimeError       `json:"lastError,omitempty"`
	Profiles     []ProfileSummary    `json:"profiles,omitempty"`
	Capabilities []RuntimeCapability `json:"capabilities,omitempty"`
}

type RuntimeCapability struct {
	Name        string            `json:"name"`
	Version     string            `json:"version,omitempty"`
	Status      RuntimeState      `json:"status"`
	Description string            `json:"description,omitempty"`
	Metadata    map[string]string `json:"metadata,omitempty"`
}
