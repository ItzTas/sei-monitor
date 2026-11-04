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

const usage = `sei-monitor acompanha os seus processos no SEI e avisa quando algo muda.

Uso: sei-monitor [subcomando]

Subcomandos:
  serve            monitora o SEI, envia as notificações e serve o feed (padrão)
  testar-login     entra no SEI e mostra as versões e as suas unidades
  orgaos           lista a sigla e o id de cada órgão da tela de login
  diagnostico      confere o comportamento da API do SEI e mostra só contagens e tipos
  enviar-teste     manda uma mensagem de teste por cada canal ligado
  whatsapp-parear  pareia o número de WhatsApp que manda as mensagens
  gerar-token      gera um valor aleatório para o FEED_TOKEN
  ajuda            mostra esta ajuda

A configuração vem de variáveis de ambiente. Veja o README.
`

var (
	errUsage          = errors.New("uso incorreto")
	errNotImplemented = errors.New("subcomando ainda não implementado")
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
		"serve":           notImplemented,
		"testar-login":    notImplemented,
		"orgaos":          notImplemented,
		"diagnostico":     notImplemented,
		"enviar-teste":    notImplemented,
		"whatsapp-parear": notImplemented,
		"gerar-token":     notImplemented,
		"ajuda":           help,
		"help":            help,
		"-h":              help,
		"--help":          help,
	}

	name, rest := "serve", args
	if len(args) > 0 {
		name, rest = args[0], args[1:]
	}
	cmd, ok := commands[name]
	if !ok {
		term.warn("subcomando desconhecido: %q\n\n%s", name, usage)
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
		term.warn("interrompido\n")
		return 130
	}
	term.warn("erro: %v\n", err)
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
