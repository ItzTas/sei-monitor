package goldentest

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"
)

const (
	regenerateCommand    = "go test ./internal/core ./internal/sei ./internal/event ./internal/monitor ./internal/cli -run TestGoldenVectors -update-golden"
	childRootVariable    = "GOLDENTEST_GOROOT"
	childVersionVariable = "GOLDENTEST_GOVERSION"
)

var (
	updateGolden bool
	childPinned  bool
	decideOnce   sync.Once
	decided      decision
)

type decision struct {
	pinned bool
	output []byte
	err    error
}

func init() {
	flag.BoolVar(&updateGolden, "update-golden", false, "rewrite the files under testdata/golden")
	root, isChild := os.LookupEnv(childRootVariable)
	if !isChild {
		return
	}
	version := os.Getenv(childVersionVariable)
	_ = os.Unsetenv(childRootVariable)
	_ = os.Unsetenv(childVersionVariable)
	if err := checkPinned(root, version); err != nil {
		fmt.Fprintf(os.Stderr, "golden vectors: %v\n", err)
		os.Exit(1)
	}
	childPinned = true
}

func Pinned(t testing.TB) bool {
	t.Helper()
	first := false
	decideOnce.Do(func() {
		first = true
		test, _, _ := strings.Cut(t.Name(), "/")
		decided = decide(test)
	})
	if decided.err != nil {
		t.Fatalf("golden vectors: %v\n%s", decided.err, decided.output)
	}
	if first && !decided.pinned && testing.Verbose() {
		t.Logf("output of the pinned child process:\n%s", decided.output)
	}
	return decided.pinned
}

func decide(test string) decision {
	if childPinned {
		return decision{pinned: true}
	}
	if err := checkToolchain(); err != nil {
		return decision{err: err}
	}
	root, version, err := goEnv()
	if err != nil {
		return decision{err: err}
	}
	if os.Getenv("ZONEINFO") != zoneFile(root) {
		output, err := runPinnedChild(test, root, version)
		return decision{output: output, err: err}
	}
	if err := checkPinned(root, version); err != nil {
		return decision{err: err}
	}
	return decision{pinned: true}
}

func runPinnedChild(test, root, version string) ([]byte, error) {
	executable, err := os.Executable()
	if err != nil {
		return nil, fmt.Errorf("find the test binary: %w", err)
	}
	dir, err := os.Getwd()
	if err != nil {
		return nil, err
	}
	args := []string{"-test.run=^" + regexp.QuoteMeta(test) + "$"}
	if testing.Verbose() {
		args = append(args, "-test.v=true")
	}
	if updateGolden {
		args = append(args, "-update-golden")
	}
	if timeout := flag.Lookup("test.timeout"); timeout != nil {
		args = append(args, "-test.timeout="+timeout.Value.String())
	}
	cmd := exec.CommandContext(context.Background(), executable, args...)
	cmd.Dir = dir
	cmd.Env = append(withoutVariables(os.Environ(), "ZONEINFO", childRootVariable, childVersionVariable),
		"ZONEINFO="+zoneFile(root), childRootVariable+"="+root, childVersionVariable+"="+version)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return output, fmt.Errorf("the pinned child process (%s %s) failed: %w", filepath.Base(executable), strings.Join(args, " "), err)
	}
	return output, nil
}

func withoutVariables(environ []string, names ...string) []string {
	return slices.DeleteFunc(slices.Clone(environ), func(entry string) bool {
		name, _, _ := strings.Cut(entry, "=")
		return slices.Contains(names, name)
	})
}

func goEnv() (string, string, error) {
	out, err := exec.CommandContext(context.Background(), "go", "env", "GOROOT", "GOVERSION").Output()
	if err != nil {
		return "", "", fmt.Errorf("run go env GOROOT GOVERSION: %w", err)
	}
	root, version, found := strings.Cut(strings.TrimSpace(string(out)), "\n")
	if !found {
		return "", "", fmt.Errorf("unexpected go env output %q", out)
	}
	return root, version, nil
}

func zoneFile(root string) string {
	return filepath.Join(root, "lib", "time", "zoneinfo.zip")
}

func pinnedGoVersion(root string) (string, error) {
	content, err := os.ReadFile(filepath.Join(root, "go.mod"))
	if err != nil {
		return "", err
	}
	for line := range strings.Lines(string(content)) {
		if version, found := strings.CutPrefix(strings.TrimSpace(line), "go "); found {
			return "go" + strings.TrimSpace(version), nil
		}
	}
	return "", errors.New("go.mod has no go line")
}

func checkToolchain() error {
	root, err := findModuleRoot()
	if err != nil {
		return err
	}
	pinned, err := pinnedGoVersion(root)
	if err != nil {
		return err
	}
	if runtime.Version() != pinned {
		return fmt.Errorf("the vectors are recorded with %s, the version in go.mod and .prototools, but this test binary was built with %s; run them with that toolchain", pinned, runtime.Version())
	}
	return nil
}

func checkPinned(root, version string) error {
	if err := checkToolchain(); err != nil {
		return err
	}
	if version != runtime.Version() {
		return fmt.Errorf("the go command on PATH is %s, but the vectors need %s (go.mod and .prototools)", version, runtime.Version())
	}
	zip := zoneFile(root)
	if zoneinfo := os.Getenv("ZONEINFO"); zoneinfo != zip {
		return fmt.Errorf("the ZONEINFO variable is %q, but the vectors read zone data only from %s", zoneinfo, zip)
	}
	if _, err := os.Stat(zip); err != nil {
		return fmt.Errorf("the zone database of the Go toolchain is missing: %w", err)
	}
	if _, err := time.LoadLocation("america/boa_vista"); err == nil {
		return errors.New("this machine resolves zone names without regard to case (america/boa_vista loads), but the vectors record Go refusing it; run them on a case-sensitive file system")
	}
	return checkZoneSource(zip)
}

func checkZoneSource(zip string) error {
	location, err := time.LoadLocation("America/Winnipeg")
	if err != nil {
		return fmt.Errorf("load America/Winnipeg: %w", err)
	}
	name, offset := time.Date(2026, 11, 2, 12, 0, 0, 0, time.UTC).In(location).Zone()
	if name != "CST" || offset != -6*60*60 {
		return fmt.Errorf("the zone data does not come from %s: America/Winnipeg on 2026-11-02 is %s %d, not CST -21600", zip, name, offset)
	}
	return nil
}

func Golden(t testing.TB, name string, value any) {
	t.Helper()
	var out bytes.Buffer
	encoder := json.NewEncoder(&out)
	encoder.SetEscapeHTML(false)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(value); err != nil {
		t.Fatalf("encode golden %s: %v", name, err)
	}
	GoldenBytes(t, name, out.Bytes())
}

func GoldenBytes(t testing.TB, name string, data []byte) {
	t.Helper()
	if !Pinned(t) {
		return
	}
	root, err := findModuleRoot()
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(root, "testdata", "golden", name)
	if updateGolden {
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile(path)
	if err != nil {
		t.Errorf("read golden %s: %v\nregenerate with: %s", name, err, regenerateCommand)
		return
	}
	if !bytes.Equal(want, data) {
		t.Errorf("golden %s differs from what the Go code produces now\nregenerate with: %s", name, regenerateCommand)
	}
}

func findModuleRoot() (string, error) {
	dir, err := os.Getwd()
	if err != nil {
		return "", err
	}
	for {
		if _, err := os.Stat(filepath.Join(dir, "go.mod")); err == nil {
			return dir, nil
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return "", errors.New("no go.mod above the working directory")
		}
		dir = parent
	}
}
