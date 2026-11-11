package cli

import (
	"bytes"
	"context"
	"log/slog"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/goldentest"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
)

const serverURLPlaceholder = "<SEI_URL>"

func TestGoldenVectors(t *testing.T) {
	goldentest.GoldenBytes(t, "diagnose.txt", []byte(diagnoseOutput(t)))
	goldentest.Golden(t, "commands.json", commandVectors(t))
}

func withoutServerURL(server *seitest.Server, text string) string {
	return strings.ReplaceAll(text, server.URL(), serverURLPlaceholder)
}

func populateDiagnose(server *seitest.Server) {
	assignee := &seitest.User{ID: 202, Acronym: "beltrano.silva", Name: "Beltrano da Silva"}
	server.SetProcesses(seitest.UnitNUPROC,
		seitest.Process{ID: 9001, Number: seitest.ProcessNumber(1), Type: "Diárias", Description: "Viagem a Brasília", ReturnDate: "06/11/2026"},
		seitest.Process{ID: 9002, Number: seitest.ProcessNumber(2), Type: "Contrato", Assignee: assignee},
		seitest.Process{ID: 9003, Number: seitest.ProcessNumber(3), Type: "Ofício", Generated: true},
	)
	at := time.Date(2026, 11, 4, 12, 0, 0, 0, time.UTC)
	server.AddActivities(9001,
		seitest.Activity{ID: 50001, User: "beltrano.silva", Unit: "SEDEMO/PROTOCOLO", Text: "Processo remetido pela unidade SEDEMO/PROTOCOLO", At: at},
		seitest.Activity{ID: 50002, User: seitest.Username, Unit: "SEDEMO/NUPROC", Text: "Processo recebido na unidade", At: at.Add(time.Hour)},
		seitest.Activity{ID: 50003, User: seitest.Username, Unit: "SEDEMO/NUPROC", Text: "Documento Ofício 12 assinado", At: at.Add(2 * time.Hour)},
	)
	server.AddDocuments(9001,
		seitest.Document{ID: 80001, Protocol: "0012345", Type: "Ofício", Number: "12", Label: "Ofício 12 (0012345)"},
		seitest.Document{ID: 80002, Protocol: "0012346", Type: "Despacho", Number: "3", Label: "Despacho 3 (0012346)"},
		seitest.Document{ID: 80003, Protocol: "0012347", Type: "Anexo", Label: "Anexo (0012347)"},
	)
}

func diagnoseOutput(t *testing.T) string {
	t.Helper()
	server := seitest.New(t)
	populateDiagnose(server)

	code, stdout, stderr := runCLI(t, seiEnv(server, nil), "diagnose")

	if code != 0 || stderr != "" {
		t.Fatalf("diagnose exit code = %d, stderr = %q", code, stderr)
	}
	return withoutServerURL(server, stdout)
}

type commandCase struct {
	Name     string            `json:"name"`
	Setup    string            `json:"setup"`
	Args     []string          `json:"args"`
	Env      map[string]string `json:"env"`
	Compare  string            `json:"compare"`
	ExitCode int               `json:"exit_code"`
	Stdout   string            `json:"stdout"`
	Stderr   string            `json:"stderr"`
}

type commandInput struct {
	name        string
	setup       string
	args        []string
	env         func(*seitest.Server) map[string]string
	compare     string
	interrupted bool
}

func commandVectors(t *testing.T) []commandCase {
	t.Helper()
	setups := map[string]func(*seitest.Server){
		"default":         func(*seitest.Server) {},
		"diagnose":        populateDiagnose,
		"module_inactive": func(server *seitest.Server) { server.SetModuleInactive(true) },
		"processes_fail_500": func(server *seitest.Server) {
			populateDiagnose(server)
			server.FailNext(seitest.RouteProcesses, http.StatusInternalServerError, 1)
		},
	}
	inputs := commandInputs()
	cases := make([]commandCase, len(inputs))
	for i, input := range inputs {
		server := seitest.New(t)
		setups[input.setup](server)
		env := input.env(server)
		code, stdout, stderr := runCommand(t, env, input.interrupted, input.args...)
		recorded := map[string]string{}
		for name, value := range env {
			recorded[name] = withoutServerURL(server, value)
		}
		cases[i] = commandCase{
			Name: input.name, Setup: input.setup, Args: input.args, Env: recorded, Compare: input.compare,
			ExitCode: code, Stdout: withoutServerURL(server, stdout), Stderr: withoutServerURL(server, stderr),
		}
	}
	return cases
}

func runCommand(t *testing.T, env map[string]string, interrupted bool, args ...string) (int, string, string) {
	t.Helper()
	if !interrupted {
		return runCLI(t, env, args...)
	}
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	var stdout, stderr bytes.Buffer
	serve := func(context.Context, core.Config, *slog.Logger) error {
		t.Error("the command started the monitor")
		return nil
	}
	code := Run(ctx, args, Terminal{Stdin: strings.NewReader(""), Stdout: &stdout, Stderr: &stderr}, envSource(env), serve)
	return code, stdout.String(), stderr.String()
}

func commandInputs() []commandInput {
	with := func(overrides map[string]string) func(*seitest.Server) map[string]string {
		return func(server *seitest.Server) map[string]string { return seiEnv(server, overrides) }
	}
	only := func(env map[string]string) func(*seitest.Server) map[string]string {
		return func(*seitest.Server) map[string]string { return env }
	}
	urlOnly := func(server *seitest.Server) map[string]string { return map[string]string{"SEI_URL": server.URL()} }
	return []commandInput{
		{name: "orgs lists every agency", setup: "default", args: []string{"orgs"}, env: urlOnly, compare: "exact"},
		{name: "orgs without SEI_URL", setup: "default", args: []string{"orgs"}, env: only(map[string]string{}), compare: "exact"},
		{name: "orgs with an unsupported SEI_URL", setup: "default", args: []string{"orgs"}, env: only(map[string]string{"SEI_URL": "ftp://sei.example.org"}), compare: "exact"},
		{name: "test-login with SEI_UNITS in other letters", setup: "default", args: []string{"test-login"}, env: with(map[string]string{"SEI_UNITS": "sedemo/gab"}), compare: "exact"},
		{name: "test-login with the current unit", setup: "default", args: []string{"test-login"}, env: with(nil), compare: "exact"},
		{name: "test-login with every unit", setup: "default", args: []string{"test-login"}, env: with(map[string]string{"SEI_UNITS": "all"}), compare: "exact"},
		{name: "test-login with the agency id", setup: "default", args: []string{"test-login"}, env: with(map[string]string{"SEI_ORG": "63"}), compare: "exact"},
		{name: "test-login with a wrong password", setup: "default", args: []string{"test-login"}, env: with(map[string]string{"SEI_PASSWORD": "wrong"}), compare: "exact"},
		{name: "test-login with a unit the user does not have", setup: "default", args: []string{"test-login"}, env: with(map[string]string{"SEI_UNITS": "SEDEMO/XYZ"}), compare: "exact"},
		{name: "test-login with an agency missing from the login page", setup: "default", args: []string{"test-login"}, env: with(map[string]string{"SEI_ORG": "NOPE"}), compare: "exact"},
		{name: "test-login with the module disabled", setup: "module_inactive", args: []string{"test-login"}, env: with(nil), compare: "exact"},
		{name: "test-login lists every configuration problem", setup: "default", args: []string{"test-login"}, env: only(map[string]string{"POLL_INTERVAL": "1m"}), compare: "exact"},
		{name: "test-login interrupted", setup: "default", args: []string{"test-login"}, env: with(nil), compare: "exact", interrupted: true},
		{name: "diagnose without processes", setup: "default", args: []string{"diagnose"}, env: with(nil), compare: "exact"},
		{name: "diagnose when the process listing fails", setup: "processes_fail_500", args: []string{"diagnose"}, env: with(nil), compare: "exact"},
		{name: "diagnose with every unit", setup: "diagnose", args: []string{"diagnose"}, env: with(map[string]string{"SEI_UNITS": "all"}), compare: "exact"},
		{name: "diagnose with a wrong password", setup: "default", args: []string{"diagnose"}, env: with(map[string]string{"SEI_PASSWORD": "wrong"}), compare: "exact"},
		{name: "diagnose with a unit the user does not have", setup: "default", args: []string{"diagnose"}, env: with(map[string]string{"SEI_UNITS": "SEDEMO/XYZ"}), compare: "exact"},
		{name: "diagnose lists every configuration problem", setup: "default", args: []string{"diagnose"}, env: only(map[string]string{}), compare: "exact"},
		{name: "unknown command", setup: "default", args: []string{"test-logn"}, env: only(map[string]string{}), compare: "usage"},
		{name: "unknown flag", setup: "default", args: []string{"orgs", "--nope"}, env: only(map[string]string{}), compare: "usage"},
		{name: "unexpected argument", setup: "default", args: []string{"orgs", "extra"}, env: only(map[string]string{}), compare: "usage"},
	}
}
