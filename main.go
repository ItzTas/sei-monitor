package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/signal"
	"runtime/debug"
	"strings"
	"syscall"

	"github.com/spf13/cobra"

	"github.com/ItzTas/sei-monitor/internal/config"
)

var version string

var errNotImplemented = errors.New("not implemented yet")

type usageError struct {
	error
}

func (e usageError) Unwrap() error {
	return e.error
}

type terminal struct {
	stdin  io.Reader
	stdout io.Writer
	stderr io.Writer
}

type app struct {
	source config.Source
}

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	code := run(ctx, os.Args[1:], terminal{stdin: os.Stdin, stdout: os.Stdout, stderr: os.Stderr}, config.Environment())
	stop()
	os.Exit(code)
}

func run(ctx context.Context, args []string, term terminal, source config.Source) int {
	root := (&app{source: source}).rootCommand()
	root.SetArgs(args)
	root.SetIn(term.stdin)
	root.SetOut(term.stdout)
	root.SetErr(term.stderr)
	return exitCode(ctx, root.ExecuteContext(ctx), term.stderr)
}

func (a *app) rootCommand() *cobra.Command {
	root := &cobra.Command{
		Use:   "sei-monitor",
		Short: "Watch your processes on SEI and get told when something changes",
		Long: "sei-monitor watches your processes on SEI and tells you when something changes.\n\n" +
			"Without a command it runs serve. Configuration comes from environment variables; see the README.",
		Version:       appVersion(),
		Args:          noArgs,
		RunE:          a.serve,
		SilenceUsage:  true,
		SilenceErrors: true,
	}
	root.CompletionOptions.HiddenDefaultCmd = true
	root.SuggestionsMinimumDistance = 2
	root.SetVersionTemplate("sei-monitor {{.Version}}\n")
	root.SetFlagErrorFunc(func(_ *cobra.Command, err error) error {
		return usageError{err}
	})
	root.AddCommand(
		command("serve", "Watch SEI, send the notifications and serve the feed (the default)", a.serve),
		command("test-login", "Log in to SEI and show the versions and your units", notImplemented),
		command("orgs", "List the acronym and id of every agency on the SEI login page", notImplemented),
		command("diagnose", "Check how the SEI API behaves, printing only counts and types", notImplemented),
		command("send-test", "Send a test message through every enabled channel", notImplemented),
		command("whatsapp-pair", "Pair the WhatsApp number that sends the messages", notImplemented),
		command("generate-token", "Generate a random value for FEED_TOKEN", notImplemented),
	)
	return root
}

func command(use, short string, run func(*cobra.Command, []string) error) *cobra.Command {
	return &cobra.Command{Use: use, Short: short, Args: noArgs, RunE: run}
}

func noArgs(cmd *cobra.Command, args []string) error {
	if len(args) == 0 {
		return nil
	}
	message := fmt.Sprintf("unknown command %q for %q", args[0], cmd.CommandPath())
	if suggestions := cmd.SuggestionsFor(args[0]); len(suggestions) > 0 {
		message += "\n\nDid you mean this?\n\t" + strings.Join(suggestions, "\n\t")
	}
	return usageError{errors.New(message)}
}

func exitCode(ctx context.Context, err error, stderr io.Writer) int {
	if err == nil {
		return 0
	}
	if usage, ok := errors.AsType[usageError](err); ok {
		_, _ = fmt.Fprintf(stderr, "%v\nRun 'sei-monitor --help' for usage.\n", usage.error)
		return 2
	}
	if ctx.Err() != nil && errors.Is(err, context.Canceled) {
		_, _ = fmt.Fprintln(stderr, "interrupted")
		return 130
	}
	_, _ = fmt.Fprintf(stderr, "error: %v\n", err)
	return 1
}

func (a *app) serve(*cobra.Command, []string) error {
	return errNotImplemented
}

func notImplemented(*cobra.Command, []string) error {
	return errNotImplemented
}

func appVersion() string {
	if version != "" {
		return version
	}
	if info, ok := debug.ReadBuildInfo(); ok && info.Main.Version != "" && info.Main.Version != "(devel)" {
		return info.Main.Version
	}
	return "dev"
}
