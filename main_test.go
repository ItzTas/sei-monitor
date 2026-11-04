package main

import (
	"bytes"
	"strings"
	"testing"
)

func runCLI(t *testing.T, args ...string) (int, string, string) {
	t.Helper()
	var stdout, stderr bytes.Buffer
	code := run(t.Context(), args, terminal{stdin: strings.NewReader(""), stdout: &stdout, stderr: &stderr})
	return code, stdout.String(), stderr.String()
}

func TestHelpPrintsUsage(t *testing.T) {
	for _, arg := range []string{"help", "-h", "--help"} {
		t.Run(arg, func(t *testing.T) {
			code, stdout, stderr := runCLI(t, arg)
			if code != 0 {
				t.Fatalf("exit code = %d, want 0", code)
			}
			if stdout != usage {
				t.Fatalf("stdout = %q, want the help", stdout)
			}
			if stderr != "" {
				t.Fatalf("stderr = %q, want nothing", stderr)
			}
		})
	}
}

func TestUnknownSubcommandFailsWithUsage(t *testing.T) {
	code, stdout, stderr := runCLI(t, "monitor")
	if code != 2 {
		t.Fatalf("exit code = %d, want 2", code)
	}
	if stdout != "" {
		t.Fatalf("stdout = %q, want nothing", stdout)
	}
	if !strings.HasPrefix(stderr, `unknown command "monitor"`) || !strings.HasSuffix(stderr, usage) {
		t.Fatalf("stderr = %q, want the unknown command followed by the help", stderr)
	}
}
