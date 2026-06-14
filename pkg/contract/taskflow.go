// SPDX-License-Identifier: AGPL-3.0-only

package contract

type TaskFlow struct {
	SchemaVersion string            `json:"schemaVersion"`
	ID            string            `json:"id"`
	Name          string            `json:"name"`
	Game          GameKey           `json:"game"`
	Servers       []ServerKey       `json:"servers"`
	Resolutions   []Resolution      `json:"resolutions"`
	Entrypoint    string            `json:"entrypoint"`
	Tasks         []TaskDefinition  `json:"tasks"`
	Metadata      map[string]string `json:"metadata,omitempty"`
}

type TaskDefinition struct {
	ID        TaskID            `json:"id"`
	Name      string            `json:"name"`
	Steps     []TaskStep        `json:"steps"`
	OnFailure FailurePolicy     `json:"onFailure"`
	Produces  []string          `json:"produces,omitempty"`
	Metadata  map[string]string `json:"metadata,omitempty"`
}

type TaskStep struct {
	ID          string         `json:"id"`
	Description string         `json:"description,omitempty"`
	Primitive   string         `json:"primitive"`
	Params      map[string]any `json:"params,omitempty"`
	When        string         `json:"when,omitempty"`
	Next        string         `json:"next,omitempty"`
	OnFailure   FailurePolicy  `json:"onFailure,omitempty"`
	TimeoutMS   int            `json:"timeoutMs,omitempty"`
}

type FailurePolicy struct {
	Severity     Severity `json:"severity"`
	RetryLimit   int      `json:"retryLimit,omitempty"`
	RetryDelayMS int      `json:"retryDelayMs,omitempty"`
	FallbackStep string   `json:"fallbackStep,omitempty"`
}
