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
	for _, arg := range []string{"ajuda", "help", "-h", "--help"} {
		t.Run(arg, func(t *testing.T) {
			code, stdout, stderr := runCLI(t, arg)
			if code != 0 {
				t.Fatalf("código de saída = %d, quero 0", code)
			}
			if stdout != usage {
				t.Fatalf("stdout = %q, quero a ajuda", stdout)
			}
			if stderr != "" {
				t.Fatalf("stderr = %q, quero vazio", stderr)
			}
		})
	}
}

func TestUnknownSubcommandFailsWithUsage(t *testing.T) {
	code, stdout, stderr := runCLI(t, "monitorar")
	if code != 2 {
		t.Fatalf("código de saída = %d, quero 2", code)
	}
	if stdout != "" {
		t.Fatalf("stdout = %q, quero vazio", stdout)
	}
	if !strings.HasPrefix(stderr, `subcomando desconhecido: "monitorar"`) || !strings.HasSuffix(stderr, usage) {
		t.Fatalf("stderr = %q, quero o subcomando desconhecido seguido da ajuda", stderr)
	}
}
