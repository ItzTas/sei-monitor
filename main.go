package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/signal"
	"syscall"
)

const usage = `sei-monitor watches your processes on SEI and tells you when something changes.

Usage: sei-monitor [command]

Commands:
  serve           watch SEI, send the notifications and serve the feed (default)
  test-login      log in to SEI and show the versions and your units
  orgs            list the acronym and id of every agency on the SEI login page
  diagnose        check how the SEI API behaves, printing only counts and types
  send-test       send a test message through every enabled channel
  whatsapp-pair   pair the WhatsApp number that sends the messages
  generate-token  generate a random value for FEED_TOKEN
  help            show this help

Configuration comes from environment variables. See the README.
`

var (
	errUsage          = errors.New("incorrect usage")
	errNotImplemented = errors.New("not implemented yet")
)

type terminal struct {
	stdin  io.Reader
	stdout io.Writer
	stderr io.Writer
}

type command func(ctx context.Context, term terminal, args []string) error

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	code := run(ctx, os.Args[1:], terminal{stdin: os.Stdin, stdout: os.Stdout, stderr: os.Stderr})
	stop()
	os.Exit(code)
}

func run(ctx context.Context, args []string, term terminal) int {
	commands := map[string]command{
		"serve":          notImplemented,
		"test-login":     notImplemented,
		"orgs":           notImplemented,
		"diagnose":       notImplemented,
		"send-test":      notImplemented,
		"whatsapp-pair":  notImplemented,
		"generate-token": notImplemented,
		"help":           help,
		"-h":             help,
		"--help":         help,
	}

	name, rest := "serve", args
	if len(args) > 0 {
		name, rest = args[0], args[1:]
	}
	cmd, ok := commands[name]
	if !ok {
		term.warn("unknown command %q\n\n%s", name, usage)
		return 2
	}

	err := cmd(ctx, term, rest)
	if err == nil {
		return 0
	}
	if errors.Is(err, errUsage) {
		term.warn("%v\n\n%s", err, usage)
		return 2
	}
	if ctx.Err() != nil && errors.Is(err, context.Canceled) {
		term.warn("interrupted\n")
		return 130
	}
	term.warn("error: %v\n", err)
	return 1
}

func (t terminal) warn(format string, args ...any) {
	_, _ = fmt.Fprintf(t.stderr, format, args...)
}

func help(_ context.Context, term terminal, _ []string) error {
	_, err := io.WriteString(term.stdout, usage)
	return err
}

func notImplemented(context.Context, terminal, []string) error {
	return errNotImplemented
}
