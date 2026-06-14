// SPDX-License-Identifier: AGPL-3.0-only

package go_bench_test

import (
	"bufio"
	"encoding/binary"
	"encoding/json"
	"io"
	"net"
	"os"
	"testing"
	"time"

	contract "github.com/HS7097/ActingCommand-Runtime/pkg/contract"
)

func readWorkload(tb testing.TB, name string) []byte {
	tb.Helper()
	data, err := os.ReadFile("../workloads/" + name)
	if err != nil {
		tb.Fatalf("read workload %s: %v", name, err)
	}
	return data
}

func sampleAcquisition() contract.AcquisitionCapture {
	return contract.AcquisitionCapture{
		ID:        "acq-20260614-000001",
		ProfileID: "alas-jp-main",
		Game:      contract.GameAzur,
		Server:    contract.ServerAlasJP,
		Locale:    "ja-JP",
		Resolution: contract.Resolution{
			Width:  1280,
			Height: 720,
			Scale:  1,
		},
		TaskID:           "daily.claim_rewards",
		TaskRunID:        "run-20260614-000001",
		CapturedAt:       time.Date(2026, 6, 14, 10, 0, 0, 0, time.UTC),
		ImageRef:         "runtime://images/acq-20260614-000001",
		ImageHash:        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
		SourceTrigger:    "reward_screen",
		RecognitionState: "pending",
		Labels:           []string{"coin", "oil", "event_point"},
		RetentionClass:   "recent",
	}
}

func BenchmarkAcquisitionJSONMarshal(b *testing.B) {
	payload := sampleAcquisition()
	b.ReportAllocs()
	for range b.N {
		if _, err := json.Marshal(payload); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkAcquisitionJSONUnmarshal(b *testing.B) {
	data := readWorkload(b, "acquisition_capture.json")
	b.ReportAllocs()
	for range b.N {
		var payload contract.AcquisitionCapture
		if err := json.Unmarshal(data, &payload); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkTaskFlowJSONUnmarshal(b *testing.B) {
	data := readWorkload(b, "task_flow.json")
	b.ReportAllocs()
	for range b.N {
		var flow contract.TaskFlow
		if err := json.Unmarshal(data, &flow); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkPrimitiveRequestBatchJSON(b *testing.B) {
	requests := []contract.CaptureRequest{
		{SessionID: "device-1", Timeout: 2 * time.Second, Reason: "benchmark"},
		{SessionID: "device-1", Region: &contract.Rect{X: 0, Y: 0, Width: 1280, Height: 720}, Timeout: time.Second, Reason: "reward_screen"},
	}
	b.ReportAllocs()
	for range b.N {
		data, err := json.Marshal(requests)
		if err != nil {
			b.Fatal(err)
		}
		var decoded []contract.CaptureRequest
		if err := json.Unmarshal(data, &decoded); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkGameEngineCommandJSON(b *testing.B) {
	payload := struct {
		Command contract.RuntimeCommand `json:"command"`
		Result  contract.CommandResult  `json:"result"`
	}{
		Command: contract.RuntimeCommand{
			ProfileID: "alas-jp-main",
			RequestID: "request-20260614-000001",
			Reason:    "benchmark_start",
			Options: map[string]string{
				"mode": "native",
			},
		},
		Result: contract.CommandResult{
			RequestID: "request-20260614-000001",
			State:     contract.RuntimeRunning,
			Accepted:  true,
			Message:   "benchmark command accepted",
		},
	}
	b.ReportAllocs()
	for range b.N {
		data, err := json.Marshal(payload)
		if err != nil {
			b.Fatal(err)
		}
		var decoded struct {
			Command contract.RuntimeCommand `json:"command"`
			Result  contract.CommandResult  `json:"result"`
		}
		if err := json.Unmarshal(data, &decoded); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkLengthPrefixedTCPRoundTrip(b *testing.B) {
	payload := readWorkload(b, "runtime_event.json")
	addr, stop := startLengthPrefixedEchoServer(b)
	defer stop()

	conn, err := net.Dial("tcp", addr)
	if err != nil {
		b.Fatal(err)
	}
	defer conn.Close()

	reader := bufio.NewReader(conn)
	b.ReportAllocs()
	b.SetBytes(int64(len(payload)))
	b.ResetTimer()
	for range b.N {
		if err := writeFrame(conn, payload); err != nil {
			b.Fatal(err)
		}
		if _, err := readFrame(reader); err != nil {
			b.Fatal(err)
		}
	}
}

func startLengthPrefixedEchoServer(tb testing.TB) (string, func()) {
	tb.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		tb.Fatal(err)
	}
	done := make(chan struct{})
	go func() {
		defer close(done)
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go handleEcho(conn)
		}
	}()
	return ln.Addr().String(), func() {
		_ = ln.Close()
		<-done
	}
}

func handleEcho(conn net.Conn) {
	defer conn.Close()
	reader := bufio.NewReader(conn)
	for {
		payload, err := readFrame(reader)
		if err != nil {
			return
		}
		if err := writeFrame(conn, payload); err != nil {
			return
		}
	}
}

func writeFrame(w io.Writer, payload []byte) error {
	var header [4]byte
	binary.BigEndian.PutUint32(header[:], uint32(len(payload)))
	if _, err := w.Write(header[:]); err != nil {
		return err
	}
	_, err := w.Write(payload)
	return err
}

func readFrame(r *bufio.Reader) ([]byte, error) {
	var header [4]byte
	if _, err := io.ReadFull(r, header[:]); err != nil {
		return nil, err
	}
	size := binary.BigEndian.Uint32(header[:])
	payload := make([]byte, size)
	_, err := io.ReadFull(r, payload)
	return payload, err
}
