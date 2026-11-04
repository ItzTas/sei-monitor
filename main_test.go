package main

import (
	"bytes"
	"io/fs"
	"os"
	"strings"
	"testing"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/store"
	"github.com/ItzTas/sei-monitor/internal/store/storetest"
)

const hardenCheck = "SEI_MONITOR_TEST_HARDEN"

func TestMain(m *testing.M) {
	if _, ok := os.LookupEnv(hardenCheck); ok {
		os.Exit(reportHardening())
	}
	os.Exit(storetest.Run(m, store.Migrate))
}

func envSource(env map[string]string) config.Source {
	return config.Source{
		Lookup: func(name string) (string, bool) {
			value, ok := env[name]
			return value, ok
		},
		ReadFile: func(string) ([]byte, error) { return nil, fs.ErrNotExist },
	}
}

func runCLI(t *testing.T, env map[string]string, args ...string) (int, string, string) {
	t.Helper()
	var stdout, stderr bytes.Buffer
	code := run(t.Context(), args, terminal{stdin: strings.NewReader(""), stdout: &stdout, stderr: &stderr}, envSource(env))
	return code, stdout.String(), stderr.String()
}

func TestHelpListsEveryCommand(t *testing.T) {
	code, stdout, stderr := runCLI(t, nil, "--help")

	if code != 0 || stderr != "" {
		t.Fatalf("exit code = %d, stderr = %q", code, stderr)
	}
	for _, name := range []string{"serve", "test-login", "orgs", "diagnose", "send-test", "whatsapp-pair", "generate-token", "Without a command it runs serve"} {
		if !strings.Contains(stdout, name) {
			t.Errorf("the help does not mention %q:\n%s", name, stdout)
		}
	}
}

func TestUnknownCommandSuggestsTheClosestOne(t *testing.T) {
	code, stdout, stderr := runCLI(t, nil, "test-logn")

	if code != 2 || stdout != "" {
		t.Fatalf("exit code = %d, stdout = %q", code, stdout)
	}
	for _, part := range []string{`unknown command "test-logn"`, "Did you mean this?", "test-login", "Run 'sei-monitor --help' for usage."} {
		if !strings.Contains(stderr, part) {
			t.Errorf("stderr = %q, want it to contain %q", stderr, part)
		}
	}
}

func TestUnknownFlagIsAUsageError(t *testing.T) {
	code, _, stderr := runCLI(t, nil, "orgs", "--nope")

	if code != 2 || !strings.Contains(stderr, "unknown flag: --nope") {
		t.Fatalf("exit code = %d, stderr = %q", code, stderr)
	}
}

func TestVersionFlag(t *testing.T) {
	code, stdout, _ := runCLI(t, nil, "--version")

	if code != 0 || stdout != "sei-monitor dev\n" {
		t.Fatalf("exit code = %d, stdout = %q", code, stdout)
	}
}
