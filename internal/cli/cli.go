package cli

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"strings"
	"time"

	"github.com/spf13/cobra"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/sei"
)

type usageError struct {
	error
}

func (e usageError) Unwrap() error {
	return e.error
}

type Terminal struct {
	Stdin  io.Reader
	Stdout io.Writer
	Stderr io.Writer
}

type Serve func(ctx context.Context, cfg core.Config, logger *slog.Logger) error

type app struct {
	source core.Source
	serve  Serve
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

func Run(ctx context.Context, args []string, term Terminal, source core.Source, serve Serve) int {
	root := (&app{source: source, serve: serve}).rootCommand()
	root.SetArgs(args)
	root.SetIn(term.Stdin)
	root.SetOut(term.Stdout)
	root.SetErr(term.Stderr)
	return exitCode(ctx, root.ExecuteContext(ctx), term.Stderr)
}

func Harden() error {
	return harden()
}

func (a *app) rootCommand() *cobra.Command {
	root := &cobra.Command{
		Use:   "sei-monitor",
		Short: "Watch your processes on SEI and get told when something changes",
		Long: "sei-monitor watches your processes on SEI and tells you when something changes.\n\n" +
			"Without a command it runs serve. Configuration comes from environment variables; see the README.",
		Version:       core.Version(),
		Args:          noArgs,
		RunE:          a.runServe,
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
	pair := command("whatsapp-pair [phone]", "Pair the WhatsApp number that sends the messages", a.whatsappPair)
	pair.Args = pairArgs
	root.AddCommand(
		openapi,
		command("serve", "Watch SEI, send the notifications and serve the feed (the default)", a.runServe),
		command("test-login", "Log in to SEI and show the versions and your units", a.testLogin),
		command("orgs", "List the acronym and id of every agency on the SEI login page", a.orgs),
		command("diagnose", "Check how the SEI API behaves, printing only counts and types", a.diagnose),
		command("send-test", "Send a test message through every enabled channel", a.sendTest),
		pair,
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

func (a *app) runServe(cmd *cobra.Command, _ []string) error {
	cfg, err := core.Load(a.source)
	if err != nil {
		return err
	}
	logger := slog.New(slog.NewTextHandler(cmd.ErrOrStderr(), &slog.HandlerOptions{Level: cfg.LogLevel}))
	return a.serve(cmd.Context(), cfg, logger)
}

func seiOptions(location *time.Location) sei.Options {
	return sei.Options{UserAgent: core.UserAgent(), Location: location}
}

func connect(ctx context.Context, cfg core.SEI) (sei.Connection, error) {
	target := sei.Target{SEIURL: cfg.URL, APIURL: cfg.APIURL, Org: cfg.Org, Username: cfg.Username, Password: cfg.Password}
	return sei.Connect(ctx, target, seiOptions(cfg.Location))
}
