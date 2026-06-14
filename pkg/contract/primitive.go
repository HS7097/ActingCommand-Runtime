// SPDX-License-Identifier: AGPL-3.0-only

package contract

import (
	"context"
	"time"
)

// PrimitiveLayer is the execution-layer boundary. It returns structured
// observations and file/content references, never raw frame buffers.
type PrimitiveLayer interface {
	ConnectDevice(context.Context, DeviceConnectRequest) (DeviceSession, error)
	StartApp(context.Context, AppRequest) (ActionResult, error)
	StopApp(context.Context, AppRequest) (ActionResult, error)
	Capture(context.Context, CaptureRequest) (CaptureRef, error)
	Match(context.Context, MatchRequest) (MatchResult, error)
	OCR(context.Context, OCRRequest) (OCRResult, error)
	GetColor(context.Context, ColorRequest) (ColorResult, error)
	Tap(context.Context, TapRequest) (ActionResult, error)
	Swipe(context.Context, SwipeRequest) (ActionResult, error)
	WaitFor(context.Context, WaitForRequest) (WaitForResult, error)
}

type DeviceConnectRequest struct {
	ProfileID ProfileID         `json:"profileId"`
	DeviceID  string            `json:"deviceId"`
	Backend   string            `json:"backend"`
	Metadata  map[string]string `json:"metadata,omitempty"`
	Timeout   time.Duration     `json:"timeout"`
}

type DeviceSession struct {
	ID          string            `json:"id"`
	DeviceID    string            `json:"deviceId"`
	Backend     string            `json:"backend"`
	Resolution  Resolution        `json:"resolution"`
	ConnectedAt time.Time         `json:"connectedAt"`
	Metadata    map[string]string `json:"metadata,omitempty"`
}

type AppRequest struct {
	SessionID string        `json:"sessionId"`
	Package   string        `json:"package"`
	Activity  string        `json:"activity,omitempty"`
	Timeout   time.Duration `json:"timeout"`
}

type CaptureRequest struct {
	SessionID string        `json:"sessionId"`
	Region    *Rect         `json:"region,omitempty"`
	Timeout   time.Duration `json:"timeout"`
	Reason    string        `json:"reason"`
}

type CaptureRef struct {
	ID         string     `json:"id"`
	ImageRef   string     `json:"imageRef"`
	ImageHash  string     `json:"imageHash,omitempty"`
	Resolution Resolution `json:"resolution"`
	CapturedAt time.Time  `json:"capturedAt"`
}

type MatchRequest struct {
	SessionID  string        `json:"sessionId"`
	CaptureID  string        `json:"captureId,omitempty"`
	Templates  []TemplateRef `json:"templates"`
	Region     *Rect         `json:"region,omitempty"`
	Threshold  float64       `json:"threshold"`
	MaxResults int           `json:"maxResults,omitempty"`
	Timeout    time.Duration `json:"timeout"`
}

type TemplateRef struct {
	ID         string     `json:"id"`
	Path       string     `json:"path"`
	Hash       string     `json:"hash,omitempty"`
	Game       GameKey    `json:"game"`
	Server     ServerKey  `json:"server"`
	Locale     string     `json:"locale,omitempty"`
	Resolution Resolution `json:"resolution"`
}

type MatchResult struct {
	Hits       []MatchHit `json:"hits"`
	ObservedAt time.Time  `json:"observedAt"`
}

type MatchHit struct {
	TemplateID string  `json:"templateId"`
	Score      float64 `json:"score"`
	Rect       Rect    `json:"rect"`
}

type OCRRequest struct {
	SessionID string        `json:"sessionId"`
	CaptureID string        `json:"captureId,omitempty"`
	Region    Rect          `json:"region"`
	Languages []string      `json:"languages,omitempty"`
	Timeout   time.Duration `json:"timeout"`
}

type OCRResult struct {
	Text       string         `json:"text"`
	Blocks     []OCRBlock     `json:"blocks,omitempty"`
	Confidence float64        `json:"confidence,omitempty"`
	ObservedAt time.Time      `json:"observedAt"`
	Warnings   []RuntimeError `json:"warnings,omitempty"`
}

type OCRBlock struct {
	Text       string  `json:"text"`
	Rect       Rect    `json:"rect"`
	Confidence float64 `json:"confidence,omitempty"`
}

type ColorRequest struct {
	SessionID string        `json:"sessionId"`
	CaptureID string        `json:"captureId,omitempty"`
	Point     Point         `json:"point"`
	Timeout   time.Duration `json:"timeout"`
}

type ColorResult struct {
	RGBA       string    `json:"rgba"`
	ObservedAt time.Time `json:"observedAt"`
}

type TapRequest struct {
	SessionID string        `json:"sessionId"`
	Point     Point         `json:"point"`
	Reason    string        `json:"reason"`
	Timeout   time.Duration `json:"timeout"`
}

type SwipeRequest struct {
	SessionID string        `json:"sessionId"`
	From      Point         `json:"from"`
	To        Point         `json:"to"`
	Duration  time.Duration `json:"duration"`
	Reason    string        `json:"reason"`
	Timeout   time.Duration `json:"timeout"`
}

type WaitForRequest struct {
	SessionID string        `json:"sessionId"`
	Condition string        `json:"condition"`
	Timeout   time.Duration `json:"timeout"`
	PollEvery time.Duration `json:"pollEvery"`
}

type WaitForResult struct {
	Satisfied  bool      `json:"satisfied"`
	ObservedAt time.Time `json:"observedAt"`
	Details    string    `json:"details,omitempty"`
}

type ActionResult struct {
	OK         bool              `json:"ok"`
	ObservedAt time.Time         `json:"observedAt"`
	Error      *RuntimeError     `json:"error,omitempty"`
	Metadata   map[string]string `json:"metadata,omitempty"`
}

type Point struct {
	X int `json:"x"`
	Y int `json:"y"`
}

type Rect struct {
	X      int `json:"x"`
	Y      int `json:"y"`
	Width  int `json:"width"`
	Height int `json:"height"`
}
