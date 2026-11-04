package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/signal"
	"runtime/debug"
	"strings"
	"syscall"
	"time"

	"github.com/spf13/cobra"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/sei"
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

type printer struct {
	w   io.Writer
	err error
}

func (p *printer) printf(format string, args ...any) {
	if p.err != nil {
		return
	}
	_, p.err = fmt.Fprintf(p.w, format, args...)
}

func main() {
	if err := harden(); err != nil {
		_, _ = fmt.Fprintf(os.Stderr, "warning: could not turn off core dumps and process inspection: %v\n", err)
	}
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
	openapi := command("openapi", "Print the OpenAPI document of the API", printOpenAPI)
	openapi.Hidden = true
	root.AddCommand(
		openapi,
		command("serve", "Watch SEI, send the notifications and serve the feed (the default)", a.serve),
		command("test-login", "Log in to SEI and show the versions and your units", a.testLogin),
		command("orgs", "List the acronym and id of every agency on the SEI login page", a.orgs),
		command("diagnose", "Check how the SEI API behaves, printing only counts and types", a.diagnose),
		command("send-test", "Send a test message through every enabled channel", a.sendTest),
		command("whatsapp-pair", "Pair the WhatsApp number that sends the messages", notImplemented),
		command("generate-token", "Print a random token to use as FEED_TOKEN or API_TOKEN (run it once for each)", generateToken),
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

func (a *app) serve(cmd *cobra.Command, _ []string) (err error) {
	cfg, err := config.Load(a.source)
	if err != nil {
		return err
	}
	logger := slog.New(slog.NewTextHandler(cmd.ErrOrStderr(), &slog.HandlerOptions{Level: cfg.LogLevel}))
	application, err := assemble(cmd.Context(), cfg, logger, time.Now)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, application.Close()) }()
	logger.Info("sei-monitor started",
		"version", appVersion(),
		"database", application.store.Dialect(),
		"interval", cfg.PollInterval,
		"channels", strings.Join(cfg.Channels.Enabled(), ","),
	)
	return application.Run(cmd.Context())
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

func userAgent() string {
	return "sei-monitor/" + appVersion()
}

func seiOptions(location *time.Location) sei.Options {
	return sei.Options{UserAgent: userAgent(), Location: location}
}

func connect(ctx context.Context, cfg config.SEI) (sei.Connection, error) {
	target := sei.Target{SEIURL: cfg.URL, APIURL: cfg.APIURL, Org: cfg.Org, Username: cfg.Username, Password: cfg.Password}
	return sei.Connect(ctx, target, seiOptions(cfg.Location))
}
