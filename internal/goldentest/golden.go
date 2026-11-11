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
	"runtime"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"
)

const regenerateCommand = "go test ./internal/core ./internal/sei ./internal/event ./internal/monitor ./internal/cli -run TestGoldenVectors -update-golden"

var (
	updateGolden bool
	goRoot       string
	goVersion    string
	setupErr     error
	checkOnce    sync.Once
	checkErr     error
)

func init() {
	flag.BoolVar(&updateGolden, "update-golden", false, "rewrite the files under testdata/golden")
	goRoot, goVersion, setupErr = goEnv()
	if setupErr == nil {
		setupErr = pinZoneData()
	}
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

func zoneFile() string {
	return filepath.Join(goRoot, "lib", "time", "zoneinfo.zip")
}

func pinZoneData() error {
	if _, set := os.LookupEnv("ZONEINFO"); set {
		return nil
	}
	executable, err := os.Executable()
	if err != nil {
		return fmt.Errorf("find the test binary to restart it with ZONEINFO: %w", err)
	}
	env := append(os.Environ(), "ZONEINFO="+zoneFile())
	err = syscall.Exec(executable, os.Args, env)
	return fmt.Errorf("restart the test binary with ZONEINFO: %w", err)
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

func checkEnvironment() error {
	if setupErr != nil {
		return setupErr
	}
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
	if goVersion != pinned {
		return fmt.Errorf("the go command on PATH is %s, but the vectors need %s (go.mod and .prototools)", goVersion, pinned)
	}
	if zoneinfo := os.Getenv("ZONEINFO"); zoneinfo != zoneFile() {
		return fmt.Errorf("the ZONEINFO variable is %q, but the vectors read zone data only from %s; unset it", zoneinfo, zoneFile())
	}
	if _, err := os.Stat(zoneFile()); err != nil {
		return fmt.Errorf("the zone database of the Go toolchain is missing: %w", err)
	}
	if _, err := time.LoadLocation("america/boa_vista"); err == nil {
		return errors.New("this machine resolves zone names without regard to case (america/boa_vista loads), but the vectors record Go refusing it; run them on a case-sensitive file system")
	}
	return checkZoneSource()
}

func checkZoneSource() error {
	location, err := time.LoadLocation("America/Winnipeg")
	if err != nil {
		return fmt.Errorf("load America/Winnipeg: %w", err)
	}
	name, offset := time.Date(2026, 11, 2, 12, 0, 0, 0, time.UTC).In(location).Zone()
	if name != "CST" || offset != -6*60*60 {
		return fmt.Errorf("the zone data does not come from %s: America/Winnipeg on 2026-11-02 is %s %d, not CST -21600", zoneFile(), name, offset)
	}
	return nil
}

func requirePinnedEnvironment(t testing.TB) {
	t.Helper()
	checkOnce.Do(func() { checkErr = checkEnvironment() })
	if checkErr != nil {
		t.Fatalf("golden vectors: %v", checkErr)
	}
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
	requirePinnedEnvironment(t)
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
