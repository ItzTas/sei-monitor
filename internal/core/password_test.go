package core

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"slices"
	"strings"
	"testing"
	"time"
)

const helperBehavior = "SEI_MONITOR_TEST_HELPER"

func TestMain(m *testing.M) {
	if behavior, ok := os.LookupEnv(helperBehavior); ok {
		os.Exit(runHelper(behavior))
	}
	os.Exit(m.Run())
}

func runHelper(behavior string) int {
	behaviors := map[string]func() int{
		"password":     func() int { fmt.Print("p4ss with spaces\n"); return 0 },
		"crlf":         func() int { fmt.Print("p4ss\r\n"); return 0 },
		"two-newlines": func() int { fmt.Print("p4ss\n\n"); return 0 },
		"no-newline":   func() int { fmt.Print("p4ss"); return 0 },
		"empty":        func() int { return 0 },
		"fail": func() int {
			fmt.Print("stdout-secret")
			fmt.Fprint(os.Stderr, "stderr-hint")
			return 3
		},
		"slow": func() int {
			time.Sleep(time.Minute)
			return 0
		},
		"environment": func() int {
			for _, entry := range os.Environ() {
				name, _, _ := strings.Cut(entry, "=")
				fmt.Println(name)
			}
			return 0
		},
		"arguments": func() int {
			fmt.Print(strings.Join(os.Args[1:], "|"))
			return 0
		},
	}
	run, ok := behaviors[behavior]
	if !ok {
		fmt.Fprintf(os.Stderr, "unknown helper behavior %q\n", behavior)
		return 2
	}
	return run()
}

func helperCommand(behavior string, extraEnv ...string) *PasswordCommand {
	return &PasswordCommand{
		args:    []string{os.Args[0]},
		timeout: 20 * time.Second,
		environ: helperEnviron(behavior, extraEnv...),
	}
}

func helperEnviron(behavior string, extraEnv ...string) func() []string {
	return func() []string {
		return append(slices.Concat(os.Environ(), extraEnv), helperBehavior+"="+behavior)
	}
}

func TestPasswordCommandUsesTheWholeStdoutMinusTheFinalNewline(t *testing.T) {
	tests := map[string]string{
		"password":     "p4ss with spaces",
		"crlf":         "p4ss",
		"two-newlines": "p4ss\n",
		"no-newline":   "p4ss",
	}
	for behavior, want := range tests {
		t.Run(behavior, func(t *testing.T) {
			t.Parallel()
			secret, err := helperCommand(behavior).Password(t.Context())
			if err != nil {
				t.Fatal(err)
			}
			if secret.Reveal() != want {
				t.Errorf("password = %q, want %q", secret.Reveal(), want)
			}
		})
	}
}

func TestPasswordCommandFailures(t *testing.T) {
	tests := []struct {
		behavior string
		timeout  time.Duration
		message  string
		check    func(*PasswordCommandError) bool
	}{
		{"empty", 20 * time.Second, "printed nothing", func(e *PasswordCommandError) bool { return e.Empty && e.ExitCode == 0 }},
		{"fail", 20 * time.Second, "exited with code 3 (11 bytes of stderr, not shown)", func(e *PasswordCommandError) bool { return e.ExitCode == 3 && e.StderrBytes == 11 }},
		{"slow", 200 * time.Millisecond, "did not finish within 200ms", func(e *PasswordCommandError) bool { return e.TimedOut }},
	}
	for _, tt := range tests {
		t.Run(tt.behavior, func(t *testing.T) {
			t.Parallel()
			command := helperCommand(tt.behavior)
			command.timeout = tt.timeout

			_, err := command.Password(t.Context())

			commandErr, ok := errors.AsType[*PasswordCommandError](err)
			if !ok || !errors.Is(err, ErrPasswordCommand) {
				t.Fatalf("error = %v, want *PasswordCommandError", err)
			}
			if !tt.check(commandErr) || !strings.Contains(err.Error(), tt.message) {
				t.Errorf("error = %q (%+v), want it to say %q", err, commandErr, tt.message)
			}
			var logs bytes.Buffer
			slog.New(slog.NewTextHandler(&logs, nil)).Error("login failed", "error", err)
			for _, output := range []string{err.Error(), fmt.Sprintf("%+v", commandErr), logs.String()} {
				if strings.Contains(output, "stdout-secret") || strings.Contains(output, "stderr-hint") {
					t.Errorf("the command output leaked into %q", output)
				}
			}
		})
	}
}

func TestPasswordCommandStopsWhenTheCallerGivesUp(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()

	_, err := helperCommand("slow").Password(ctx)

	if !errors.Is(err, context.Canceled) || errors.Is(err, ErrPasswordCommand) {
		t.Fatalf("error = %v, want context.Canceled", err)
	}
}

func TestPasswordCommandEnvironmentHasNoSecrets(t *testing.T) {
	secrets := []string{
		"SEI_PASSWORD", "SEI_PASSWORD_FILE", "SEI_PASSWORD_CMD", "FEED_TOKEN", "FEED_TOKEN_FILE", "API_TOKEN",
		"SMTP_PASSWORD", "TELEGRAM_TOKEN", "NTFY_TOKEN_FILE", "WEBHOOK_SECRET", "POSTGRES_PASSWORD", "DATABASE_URL",
	}
	kept := []string{"VAULT_TOKEN", "BW_SESSION", "SEI_USERNAME"}
	var extraEnv []string
	for _, name := range slices.Concat(secrets, kept) {
		extraEnv = append(extraEnv, name+"=value-of-"+name)
	}

	secret, err := helperCommand("environment", extraEnv...).Password(t.Context())
	if err != nil {
		t.Fatal(err)
	}

	names := strings.Fields(secret.Reveal())
	for _, name := range secrets {
		if slices.Contains(names, name) {
			t.Errorf("the command received %s", name)
		}
	}
	for _, name := range kept {
		if !slices.Contains(names, name) {
			t.Errorf("the command did not receive %s, which password managers may need", name)
		}
	}
}

func TestPasswordCommandSplitsArgumentsWithoutAShell(t *testing.T) {
	line := fmt.Sprintf(`'%s' plain "two words" 'single "quoted"' esc\ aped "back\\slash" "$HOME"`, os.Args[0])
	command, err := NewPasswordCommand(line)
	if err != nil {
		t.Fatal(err)
	}
	command.environ = helperEnviron("arguments")

	secret, err := command.Password(t.Context())
	if err != nil {
		t.Fatal(err)
	}

	want := `plain|two words|single "quoted"|esc aped|back\slash|$HOME`
	if secret.Reveal() != want {
		t.Errorf("arguments = %q, want %q", secret.Reveal(), want)
	}
}

func TestSplitCommand(t *testing.T) {
	tests := []struct {
		line string
		want []string
	}{
		{"pass show sei/monitor", []string{"pass", "show", "sei/monitor"}},
		{"  secret-tool   lookup service sei  ", []string{"secret-tool", "lookup", "service", "sei"}},
		{`bw get password "sei monitor"`, []string{"bw", "get", "password", "sei monitor"}},
		{`cmd ''`, []string{"cmd", ""}},
		{`cmd "a\"b"`, []string{"cmd", `a"b`}},
		{`cmd 'it''s'`, []string{"cmd", "its"}},
	}
	for _, tt := range tests {
		got, err := splitCommand(tt.line)
		if err != nil || !slices.Equal(got, tt.want) {
			t.Errorf("splitCommand(%q) = %q, %v; want %q", tt.line, got, err, tt.want)
		}
	}
	for _, line := range []string{"", "   ", `cmd "open`, "cmd 'open", `cmd trailing\`} {
		if _, err := splitCommand(line); err == nil {
			t.Errorf("splitCommand(%q) returned no error", line)
		}
	}
}
