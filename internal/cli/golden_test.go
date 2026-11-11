package cli

import (
	"bytes"
	"context"
	"log/slog"
	"maps"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/goldentest"
	"github.com/ItzTas/sei-monitor/internal/goldentest/seisetup"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
)

const serverURLPlaceholder = "<SEI_URL>"

func TestGoldenVectors(t *testing.T) {
	if !goldentest.Pinned(t) {
		return
	}
	commands := commandVectors(t)
	goldentest.GoldenBytes(t, "diagnose.txt", []byte(diagnoseOutput(t, commands)))
	goldentest.Golden(t, "commands.json", commands)
}

const diagnoseCaseName = "diagnose with the scenario of diagnose.txt"

func withoutServerURL(server *seitest.Server, text string) string {
	return strings.ReplaceAll(text, server.URL(), serverURLPlaceholder)
}

func withServerURL(server *seitest.Server, env map[string]string) map[string]string {
	resolved := make(map[string]string, len(env))
	for name, value := range env {
		resolved[name] = strings.ReplaceAll(value, serverURLPlaceholder, server.URL())
	}
	return resolved
}

func diagnoseSetup() []seisetup.Call {
	assignee := &seisetup.User{ID: 202, Acronym: "beltrano.silva", Name: "Beltrano da Silva"}
	at := time.Date(2026, 11, 4, 12, 0, 0, 0, time.UTC)
	return []seisetup.Call{
		seisetup.SetProcesses(seitest.UnitNUPROC,
			seisetup.Process{ID: 9001, Number: seitest.ProcessNumber(1), Type: "Diárias", Description: "Viagem a Brasília", ReturnDate: "06/11/2026"},
			seisetup.Process{ID: 9002, Number: seitest.ProcessNumber(2), Type: "Contrato", Assignee: assignee},
			seisetup.Process{ID: 9003, Number: seitest.ProcessNumber(3), Type: "Ofício", Generated: true},
		),
		seisetup.AddActivities(9001,
			seisetup.Activity{ID: 50001, User: "beltrano.silva", Unit: "SEDEMO/PROTOCOLO", Text: "Processo remetido pela unidade SEDEMO/PROTOCOLO", At: seisetup.At(at)},
			seisetup.Activity{ID: 50002, User: seitest.Username, Unit: "SEDEMO/NUPROC", Text: "Processo recebido na unidade", At: seisetup.At(at.Add(time.Hour))},
			seisetup.Activity{ID: 50003, User: seitest.Username, Unit: "SEDEMO/NUPROC", Text: "Documento Ofício 12 assinado", At: seisetup.At(at.Add(2 * time.Hour))},
		),
		seisetup.AddDocuments(9001,
			seisetup.Document{ID: 80001, Protocol: "0012345", Type: "Ofício", Number: "12", Label: "Ofício 12 (0012345)"},
			seisetup.Document{ID: 80002, Protocol: "0012346", Type: "Despacho", Number: "3", Label: "Despacho 3 (0012346)"},
			seisetup.Document{ID: 80003, Protocol: "0012347", Type: "Anexo", Label: "Anexo (0012347)"},
		),
	}
}

func seiSettings(overrides map[string]string) map[string]string {
	env := map[string]string{
		"SEI_URL":      serverURLPlaceholder,
		"SEI_USERNAME": seitest.Username,
		"SEI_PASSWORD": seitest.Password,
		"SEI_ORG":      seitest.OrgAcronym,
		"SEI_TIMEZONE": seitest.TimeZone,
	}
	maps.Copy(env, overrides)
	return env
}

func diagnoseOutput(t *testing.T, commands commandFile) string {
	t.Helper()
	for _, c := range commands.Cases {
		if c.Name != diagnoseCaseName {
			continue
		}
		if c.ExitCode != 0 || c.Stderr != "" {
			t.Fatalf("diagnose exit code = %d, stderr = %q", c.ExitCode, c.Stderr)
		}
		return c.Stdout
	}
	t.Fatalf("no command case named %q", diagnoseCaseName)
	return ""
}

type interrupt struct {
	When string `json:"when"`
	Note string `json:"note"`
}

type commandCase struct {
	Name      string            `json:"name"`
	Setup     []seisetup.Call   `json:"setup"`
	Args      []string          `json:"args"`
	Env       map[string]string `json:"env"`
	Interrupt *interrupt        `json:"interrupt"`
	Compare   string            `json:"compare"`
	ExitCode  int               `json:"exit_code"`
	Stdout    string            `json:"stdout"`
	Stderr    string            `json:"stderr"`
}

type commandFile struct {
	Notes    string            `json:"notes"`
	Defaults seisetup.Defaults `json:"defaults"`
	Cases    []commandCase     `json:"cases"`
}

func commandVectors(t *testing.T) commandFile {
	t.Helper()
	defaults, err := seisetup.ReadDefaults(t.Context(), seitest.New(t))
	if err != nil {
		t.Fatal(err)
	}
	cases := commandInputs()
	for i := range cases {
		c := &cases[i]
		server := seitest.New(t)
		if err := seisetup.Apply(server, c.Setup); err != nil {
			t.Fatal(err)
		}
		code, stdout, stderr := runCommand(t, withServerURL(server, c.Env), c.Interrupt != nil, c.Args...)
		c.ExitCode, c.Stdout, c.Stderr = code, withoutServerURL(server, stdout), withoutServerURL(server, stderr)
	}
	notes := "Each case starts on a fresh fake SEI server (these commands use no database), applies its setup calls in order, and runs the command with exactly the recorded environment, nothing inherited; <SEI_URL> stands for the fake's base URL."
	return commandFile{Notes: notes, Defaults: defaults, Cases: cases}
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

func commandInputs() []commandCase {
	none := []seisetup.Call{}
	failing := append(diagnoseSetup(), seisetup.FailNext(seitest.RouteProcesses, http.StatusInternalServerError, 1))
	beforeFirstRequest := &interrupt{
		When: "before_first_request",
		Note: "Go ran the command with a context cancelled before it started. The configuration is valid, so it is read; " +
			"the first request to SEI then fails with the cancellation. A replay sends SIGINT after the process has started " +
			"and before the fake SEI answers its first request (hold that answer until the signal is delivered). " +
			"test-login prints nothing before every call has returned, so stdout is empty for any interrupt that lands before the last answer.",
	}
	command := func(name string, setup []seisetup.Call, env map[string]string, compare string, args ...string) commandCase {
		return commandCase{Name: name, Setup: setup, Args: args, Env: env, Compare: compare}
	}
	interrupted := command("test-login interrupted", none, seiSettings(nil), "exact", "test-login")
	interrupted.Interrupt = beforeFirstRequest
	return []commandCase{
		command("orgs lists every agency", none, map[string]string{"SEI_URL": serverURLPlaceholder}, "exact", "orgs"),
		command("orgs without SEI_URL", none, map[string]string{}, "exact", "orgs"),
		command("orgs with an unsupported SEI_URL", none, map[string]string{"SEI_URL": "ftp://sei.example.org"}, "exact", "orgs"),
		command("test-login with SEI_UNITS in other letters", none, seiSettings(map[string]string{"SEI_UNITS": "sedemo/gab"}), "exact", "test-login"),
		command("test-login with the current unit", none, seiSettings(nil), "exact", "test-login"),
		command("test-login with every unit", none, seiSettings(map[string]string{"SEI_UNITS": "all"}), "exact", "test-login"),
		command("test-login with the agency id", none, seiSettings(map[string]string{"SEI_ORG": "63"}), "exact", "test-login"),
		command("test-login with a wrong password", none, seiSettings(map[string]string{"SEI_PASSWORD": "wrong"}), "exact", "test-login"),
		command("test-login with a unit the user does not have", none, seiSettings(map[string]string{"SEI_UNITS": "SEDEMO/XYZ"}), "exact", "test-login"),
		command("test-login with an agency missing from the login page", none, seiSettings(map[string]string{"SEI_ORG": "NOPE"}), "exact", "test-login"),
		command("test-login with the module disabled", []seisetup.Call{seisetup.SetModuleInactive(true)}, seiSettings(nil), "exact", "test-login"),
		command("test-login lists every configuration problem", none, map[string]string{"POLL_INTERVAL": "1m"}, "exact", "test-login"),
		interrupted,
		command("diagnose without processes", none, seiSettings(nil), "exact", "diagnose"),
		command("diagnose when the process listing fails", failing, seiSettings(nil), "exact", "diagnose"),
		command("diagnose with every unit", diagnoseSetup(), seiSettings(map[string]string{"SEI_UNITS": "all"}), "exact", "diagnose"),
		command("diagnose with a wrong password", none, seiSettings(map[string]string{"SEI_PASSWORD": "wrong"}), "exact", "diagnose"),
		command("diagnose with a unit the user does not have", none, seiSettings(map[string]string{"SEI_UNITS": "SEDEMO/XYZ"}), "exact", "diagnose"),
		command("diagnose lists every configuration problem", none, map[string]string{}, "exact", "diagnose"),
		command("unknown command", none, map[string]string{}, "usage", "test-logn"),
		command("unknown flag", none, map[string]string{}, "usage", "orgs", "--nope"),
		command("unexpected argument", none, map[string]string{}, "usage", "orgs", "extra"),
		command(diagnoseCaseName, diagnoseSetup(), seiSettings(nil), "exact", "diagnose"),
	}
}
