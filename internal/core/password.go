package core

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"
	"unicode"
)

const passwordCommandTimeout = 30 * time.Second

var ErrPasswordCommand = errors.New("the password command failed")

type PasswordSource interface {
	Password(ctx context.Context) (Secret, error)
}

type staticPassword struct {
	secret Secret
}

func StaticPassword(secret Secret) PasswordSource {
	return staticPassword{secret: secret}
}

func (p staticPassword) Password(context.Context) (Secret, error) {
	return p.secret, nil
}

type PasswordCommand struct {
	args    []string
	timeout time.Duration
	environ func() []string
}

func NewPasswordCommand(line string) (*PasswordCommand, error) {
	args, err := splitCommand(line)
	if err != nil {
		return nil, err
	}
	return &PasswordCommand{args: args, timeout: passwordCommandTimeout, environ: os.Environ}, nil
}

func (c *PasswordCommand) Program() string {
	return c.args[0]
}

func (c *PasswordCommand) Password(ctx context.Context) (Secret, error) {
	runCtx, cancel := context.WithTimeout(ctx, c.timeout)
	defer cancel()

	var stdout, stderr bytes.Buffer
	cmd := exec.CommandContext(runCtx, c.args[0], c.args[1:]...)
	cmd.Env = withoutSecrets(c.environ())
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	cmd.WaitDelay = time.Second
	err := cmd.Run()

	failure := &PasswordCommandError{Program: c.args[0], ExitCode: -1, StderrBytes: stderr.Len(), timeout: c.timeout}
	if ctx.Err() != nil {
		return Secret{}, ctx.Err()
	}
	if runCtx.Err() != nil {
		failure.TimedOut = true
		return Secret{}, failure
	}
	if exitErr, ok := errors.AsType[*exec.ExitError](err); ok {
		failure.ExitCode = exitErr.ExitCode()
		return Secret{}, failure
	}
	if err != nil {
		failure.startErr = err
		return Secret{}, failure
	}
	password := trimFinalNewline(stdout.String())
	if password == "" {
		failure.ExitCode = 0
		failure.Empty = true
		return Secret{}, failure
	}
	return NewSecret(password), nil
}

type PasswordCommandError struct {
	Program     string
	ExitCode    int
	StderrBytes int
	TimedOut    bool
	Empty       bool
	timeout     time.Duration
	startErr    error
}

func (e *PasswordCommandError) Error() string {
	reasons := []struct {
		matches bool
		message string
	}{
		{e.TimedOut, fmt.Sprintf("%s did not finish within %s", e.Program, e.timeout)},
		{e.startErr != nil, fmt.Sprintf("could not run %s: %v", e.Program, e.startErr)},
		{e.Empty, fmt.Sprintf("%s printed nothing", e.Program)},
		{true, fmt.Sprintf("%s exited with code %d", e.Program, e.ExitCode)},
	}
	message := ""
	for _, reason := range reasons {
		if reason.matches {
			message = reason.message
			break
		}
	}
	if e.StderrBytes > 0 {
		message += fmt.Sprintf(" (%d bytes of stderr, not shown)", e.StderrBytes)
	}
	return "SEI_PASSWORD_CMD: " + message
}

func (e *PasswordCommandError) Is(target error) bool {
	return target == ErrPasswordCommand
}

func trimFinalNewline(text string) string {
	if trimmed, ok := strings.CutSuffix(text, "\r\n"); ok {
		return trimmed
	}
	return strings.TrimSuffix(text, "\n")
}

func withoutSecrets(environ []string) []string {
	secrets := map[string]bool{
		"SEI_PASSWORD":      true,
		"FEED_TOKEN":        true,
		"API_TOKEN":         true,
		"SMTP_PASSWORD":     true,
		"TELEGRAM_TOKEN":    true,
		"NTFY_TOKEN":        true,
		"WEBHOOK_SECRET":    true,
		"POSTGRES_PASSWORD": true,
		"DATABASE_URL":      true,
	}
	kept := make([]string, 0, len(environ))
	for _, entry := range environ {
		name, _, _ := strings.Cut(entry, "=")
		base := strings.TrimSuffix(strings.TrimSuffix(strings.ToUpper(name), "_FILE"), "_CMD")
		if !secrets[base] {
			kept = append(kept, entry)
		}
	}
	return kept
}

func splitCommand(line string) ([]string, error) {
	var (
		args    []string
		current strings.Builder
		started bool
		quote   rune
		escaped bool
	)
	for _, r := range line {
		switch {
		case escaped:
			current.WriteRune(r)
			escaped = false
		case quote == '\'' && r == '\'':
			quote = 0
		case quote == '\'':
			current.WriteRune(r)
		case quote == '"' && r == '"':
			quote = 0
		case r == '\\':
			escaped, started = true, true
		case quote == '"':
			current.WriteRune(r)
		case r == '\'' || r == '"':
			quote, started = r, true
		case unicode.IsSpace(r):
			if started {
				args = append(args, current.String())
				current.Reset()
				started = false
			}
		default:
			current.WriteRune(r)
			started = true
		}
	}
	if quote != 0 || escaped {
		return nil, errors.New("SEI_PASSWORD_CMD: unterminated quote or trailing backslash")
	}
	if started {
		args = append(args, current.String())
	}
	if len(args) == 0 {
		return nil, errors.New("SEI_PASSWORD_CMD is empty")
	}
	return args, nil
}
