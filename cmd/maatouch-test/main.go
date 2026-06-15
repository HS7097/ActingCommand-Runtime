package main

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

type config struct {
	adbPath         string
	serial          string
	host            string
	port            int
	localMaaTouch   string
	remoteMaaTouch  string
	connect         bool
	push            bool
	tap             bool
	x               int
	y               int
	pressure        int
	commandTimeout  time.Duration
	handshakeWait   time.Duration
	shutdownTimeout time.Duration
}

type handshakeInfo struct {
	maxContacts int
	maxX        int
	maxY        int
	maxPressure int
	pid         string
	lines       []string
}

type commandOutput struct {
	stdout string
	stderr string
}

type lockedBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *lockedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *lockedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.String()
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintf(os.Stderr, "maatouch-test failed: %v\n", err)
		os.Exit(1)
	}
}

func run() error {
	cfg := parseFlags()
	if cfg.serial == "" {
		cfg.serial = fmt.Sprintf("%s:%d", cfg.host, cfg.port)
	}

	fmt.Printf("Target device: %s\n", cfg.serial)
	fmt.Printf("Local MaaTouch: %s\n", cfg.localMaaTouch)
	fmt.Printf("Remote MaaTouch: %s\n", cfg.remoteMaaTouch)

	if cfg.push {
		if err := requireFile(cfg.localMaaTouch); err != nil {
			return err
		}
	}

	ctx, cancel := context.WithTimeout(context.Background(), cfg.commandTimeout)
	defer cancel()

	if cfg.connect {
		out, err := runADB(ctx, cfg.adbPath, "connect", cfg.serial)
		if err != nil {
			return fmt.Errorf("adb connect failed: %w", err)
		}
		printCommandOutput("adb connect", out)
	}

	if err := verifyDevice(ctx, cfg); err != nil {
		return err
	}

	if cfg.push {
		if err := pushMaaTouch(ctx, cfg); err != nil {
			return err
		}
	}

	info, err := runMaaTouchSession(cfg)
	if err != nil {
		return err
	}

	fmt.Printf("MaaTouch handshake OK: contacts=%d size=%dx%d pressure=%d pid=%s\n",
		info.maxContacts, info.maxX, info.maxY, info.maxPressure, info.pid)
	fmt.Println("PASS")
	return nil
}

func parseFlags() config {
	cfg := config{
		adbPath:         "adb",
		host:            "127.0.0.1",
		port:            16384,
		localMaaTouch:   defaultMaaTouchPath(),
		remoteMaaTouch:  "/data/local/tmp/maatouch",
		connect:         true,
		push:            true,
		tap:             false,
		x:               640,
		y:               360,
		pressure:        50,
		commandTimeout:  12 * time.Second,
		handshakeWait:   8 * time.Second,
		shutdownTimeout: 1 * time.Second,
	}

	flag.StringVar(&cfg.adbPath, "adb", cfg.adbPath, "adb executable path")
	flag.StringVar(&cfg.serial, "serial", cfg.serial, "adb serial; defaults to host:port")
	flag.StringVar(&cfg.host, "host", cfg.host, "emulator adb host")
	flag.IntVar(&cfg.port, "port", cfg.port, "emulator adb port")
	flag.StringVar(&cfg.localMaaTouch, "local", cfg.localMaaTouch, "local MaaTouch file to push")
	flag.StringVar(&cfg.remoteMaaTouch, "remote", cfg.remoteMaaTouch, "remote MaaTouch path")
	flag.BoolVar(&cfg.connect, "connect", cfg.connect, "run adb connect before testing")
	flag.BoolVar(&cfg.push, "push", cfg.push, "push MaaTouch before testing")
	flag.BoolVar(&cfg.tap, "tap", cfg.tap, "send one tap after the MaaTouch handshake")
	flag.IntVar(&cfg.x, "x", cfg.x, "tap x coordinate when -tap is enabled")
	flag.IntVar(&cfg.y, "y", cfg.y, "tap y coordinate when -tap is enabled")
	flag.IntVar(&cfg.pressure, "pressure", cfg.pressure, "tap pressure when -tap is enabled")
	flag.DurationVar(&cfg.commandTimeout, "command-timeout", cfg.commandTimeout, "timeout for short adb commands")
	flag.DurationVar(&cfg.handshakeWait, "handshake-timeout", cfg.handshakeWait, "timeout while waiting for MaaTouch startup")
	flag.DurationVar(&cfg.shutdownTimeout, "shutdown-timeout", cfg.shutdownTimeout, "timeout while stopping MaaTouch test process")
	flag.Parse()

	return cfg
}

func defaultMaaTouchPath() string {
	return filepath.Clean(filepath.Join("..", "upstream-sources", "AzurPilot", "bin", "MaaTouch", "maatouch"))
}

func requireFile(path string) error {
	info, err := os.Stat(path)
	if err != nil {
		return fmt.Errorf("required MaaTouch file is unavailable at %s: %w", path, err)
	}
	if info.IsDir() {
		return fmt.Errorf("required MaaTouch path is a directory: %s", path)
	}
	return nil
}

func verifyDevice(ctx context.Context, cfg config) error {
	state, err := runADB(ctx, cfg.adbPath, "-s", cfg.serial, "get-state")
	if err != nil {
		devices, listErr := runADB(ctx, cfg.adbPath, "devices", "-l")
		if listErr != nil {
			return fmt.Errorf("target device %s is not available; adb devices also failed: %w", cfg.serial, listErr)
		}
		return fmt.Errorf("target device %s is not available: %w\nadb devices -l:\n%s", cfg.serial, err, devices.stdout)
	}
	if strings.TrimSpace(state.stdout) != "device" {
		return fmt.Errorf("target device %s is not in device state: %q", cfg.serial, strings.TrimSpace(state.stdout))
	}

	size, err := runADB(ctx, cfg.adbPath, "-s", cfg.serial, "shell", "wm", "size")
	if err != nil {
		return fmt.Errorf("failed to read device screen size for %s: %w", cfg.serial, err)
	}
	fmt.Printf("Device state: %s\n", strings.TrimSpace(state.stdout))
	fmt.Printf("Device screen: %s\n", strings.TrimSpace(size.stdout))
	return nil
}

func pushMaaTouch(ctx context.Context, cfg config) error {
	out, err := runADB(ctx, cfg.adbPath, "-s", cfg.serial, "push", cfg.localMaaTouch, cfg.remoteMaaTouch)
	if err != nil {
		return fmt.Errorf("failed to push MaaTouch to %s: %w", cfg.remoteMaaTouch, err)
	}
	printCommandOutput("adb push", out)

	out, err = runADB(ctx, cfg.adbPath, "-s", cfg.serial, "shell", "chmod", "755", cfg.remoteMaaTouch)
	if err != nil {
		return fmt.Errorf("failed to chmod MaaTouch at %s: %w", cfg.remoteMaaTouch, err)
	}
	printCommandOutput("adb chmod", out)
	return nil
}

func runMaaTouchSession(cfg config) (handshakeInfo, error) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	cmd := exec.CommandContext(
		ctx,
		cfg.adbPath,
		"-s", cfg.serial,
		"shell",
		"CLASSPATH="+cfg.remoteMaaTouch,
		"app_process",
		"/",
		"com.shxyke.MaaTouch.App",
	)

	stdin, err := cmd.StdinPipe()
	if err != nil {
		return handshakeInfo{}, fmt.Errorf("failed to open MaaTouch stdin: %w", err)
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return handshakeInfo{}, fmt.Errorf("failed to open MaaTouch stdout: %w", err)
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return handshakeInfo{}, fmt.Errorf("failed to open MaaTouch stderr: %w", err)
	}

	var stderrBuffer lockedBuffer
	if err := cmd.Start(); err != nil {
		return handshakeInfo{}, fmt.Errorf("failed to start MaaTouch app_process: %w", err)
	}

	waitCh := make(chan error, 1)
	go func() {
		waitCh <- cmd.Wait()
	}()
	go func() {
		_, _ = io.Copy(&stderrBuffer, stderr)
	}()

	reader := bufio.NewReader(stdout)
	handshakeCh := make(chan handshakeResult, 1)
	go func() {
		info, err := readHandshake(reader)
		handshakeCh <- handshakeResult{info: info, err: err}
	}()

	var info handshakeInfo
	select {
	case result := <-handshakeCh:
		if result.err != nil {
			stopMaaTouchProcess(stdin, cancel, waitCh, cfg.shutdownTimeout)
			return handshakeInfo{}, attachStderr(result.err, stderrBuffer.String())
		}
		info = result.info
	case err := <-waitCh:
		return handshakeInfo{}, attachStderr(fmt.Errorf("MaaTouch exited before handshake: %w", err), stderrBuffer.String())
	case <-time.After(cfg.handshakeWait):
		stopMaaTouchProcess(stdin, cancel, waitCh, cfg.shutdownTimeout)
		return handshakeInfo{}, attachStderr(fmt.Errorf("timed out after %s waiting for MaaTouch handshake", cfg.handshakeWait), stderrBuffer.String())
	}

	if err := sendReset(stdin); err != nil {
		stopMaaTouchProcess(stdin, cancel, waitCh, cfg.shutdownTimeout)
		return handshakeInfo{}, fmt.Errorf("failed to send MaaTouch reset command: %w", err)
	}

	if cfg.tap {
		if err := sendTap(stdin, cfg.x, cfg.y, cfg.pressure); err != nil {
			stopMaaTouchProcess(stdin, cancel, waitCh, cfg.shutdownTimeout)
			return handshakeInfo{}, fmt.Errorf("failed to send MaaTouch tap command: %w", err)
		}
		fmt.Printf("Tap sent: x=%d y=%d pressure=%d\n", cfg.x, cfg.y, cfg.pressure)
	} else {
		fmt.Println("Tap skipped: pass -tap to send one down/up touch event.")
	}

	stopMaaTouchProcess(stdin, cancel, waitCh, cfg.shutdownTimeout)
	if stderrText := strings.TrimSpace(stderrBuffer.String()); stderrText != "" {
		if stderrText == "Killed" {
			fmt.Println("MaaTouch process stopped after validation (stderr: Killed).")
		} else {
			fmt.Fprintf(os.Stderr, "MaaTouch stderr:\n%s\n", stderrText)
		}
	}
	return info, nil
}

type handshakeResult struct {
	info handshakeInfo
	err  error
}

func readHandshake(reader *bufio.Reader) (handshakeInfo, error) {
	var lines []string
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			return handshakeInfo{lines: lines}, fmt.Errorf("failed to read MaaTouch handshake: %w", err)
		}
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		lines = append(lines, line)
		if line == "Aborted" {
			return handshakeInfo{lines: lines}, errors.New("MaaTouch reported Aborted during startup")
		}
		if !strings.HasPrefix(line, "^ ") {
			continue
		}

		info, err := parseVersionLine(line)
		if err != nil {
			return handshakeInfo{lines: lines}, err
		}
		info.lines = lines

		pidLine, err := reader.ReadString('\n')
		if err != nil {
			return info, fmt.Errorf("failed to read MaaTouch pid line after version: %w", err)
		}
		pidLine = strings.TrimSpace(pidLine)
		info.lines = append(info.lines, pidLine)
		if strings.HasPrefix(pidLine, "$ ") {
			info.pid = strings.TrimSpace(strings.TrimPrefix(pidLine, "$ "))
			return info, nil
		}
		return info, fmt.Errorf("unexpected MaaTouch pid line: %q", pidLine)
	}
}

func parseVersionLine(line string) (handshakeInfo, error) {
	fields := strings.Fields(line)
	if len(fields) != 5 || fields[0] != "^" {
		return handshakeInfo{}, fmt.Errorf("invalid MaaTouch version line: %q", line)
	}
	values := make([]int, 4)
	for i := 1; i < len(fields); i++ {
		value, err := strconv.Atoi(fields[i])
		if err != nil {
			return handshakeInfo{}, fmt.Errorf("invalid MaaTouch version field %q in %q: %w", fields[i], line, err)
		}
		values[i-1] = value
	}
	return handshakeInfo{
		maxContacts: values[0],
		maxX:        values[1],
		maxY:        values[2],
		maxPressure: values[3],
	}, nil
}

func sendReset(stdin io.Writer) error {
	if _, err := io.WriteString(stdin, "r\nc\n"); err != nil {
		return err
	}
	fmt.Println("Reset command sent.")
	return nil
}

func sendTap(stdin io.Writer, x int, y int, pressure int) error {
	if _, err := fmt.Fprintf(stdin, "d 0 %d %d %d\nc\n", x, y, pressure); err != nil {
		return err
	}
	time.Sleep(80 * time.Millisecond)
	_, err := io.WriteString(stdin, "u 0\nc\n")
	return err
}

func stopMaaTouchProcess(stdin io.Closer, cancel context.CancelFunc, waitCh <-chan error, timeout time.Duration) {
	_ = stdin.Close()
	select {
	case <-waitCh:
		return
	case <-time.After(timeout):
		cancel()
	}
	select {
	case <-waitCh:
	case <-time.After(timeout):
		fmt.Fprintf(os.Stderr, "warning: MaaTouch process did not exit within %s after cancellation\n", timeout)
	}
}

func runADB(ctx context.Context, adbPath string, args ...string) (commandOutput, error) {
	cmd := exec.CommandContext(ctx, adbPath, args...)
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	err := cmd.Run()
	out := commandOutput{stdout: stdout.String(), stderr: stderr.String()}
	if err != nil {
		return out, fmt.Errorf("adb %s failed: %w\nstdout:\n%s\nstderr:\n%s", strings.Join(args, " "), err, out.stdout, out.stderr)
	}
	return out, nil
}

func printCommandOutput(label string, out commandOutput) {
	if text := strings.TrimSpace(out.stdout); text != "" {
		fmt.Printf("%s stdout: %s\n", label, text)
	}
	if text := strings.TrimSpace(out.stderr); text != "" {
		fmt.Fprintf(os.Stderr, "%s stderr: %s\n", label, text)
	}
}

func attachStderr(err error, stderr string) error {
	stderr = strings.TrimSpace(stderr)
	if stderr == "" {
		return err
	}
	return fmt.Errorf("%w\nMaaTouch stderr:\n%s", err, stderr)
}
