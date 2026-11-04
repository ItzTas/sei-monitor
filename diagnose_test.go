package main

import (
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
)

func TestDiagnosePrintsOnlyCountsAndTypes(t *testing.T) {
	server := seitest.New(t)
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

	code, stdout, stderr := runCLI(t, seiEnv(server, nil), "diagnose")

	if code != 0 {
		t.Fatalf("exit code = %d, stderr = %q", code, stderr)
	}
	want := []string{
		"SEI 5.0.5, mod-wssei 3.0.4; units: 2 available, 1 watched",
		"/processo/listar: page 0 has 2 ids, page 1 has 1, 0 in common -> start is a page number",
		"/atividade/listar: page 0 has 2 ids, page 1 has 1, 0 in common -> start is a page number",
		"/documento/listar: page 0 has 2 ids, page 1 has 1, 0 in common -> start is a page number",
		"default call: total 3; G=1 R=2 other=0",
		"tipo=R:       total 2; G=0 R=2 other=0",
		"tipo=G:       total 1; G=1 R=0 other=0",
		"retornoProgramado=S: 1; retornoData: object{dataProgramada,unidade} x1",
		"dataProgramada: 99/99/9999 x1",
		"retornoProgramado=N with retornoData filled: 2",
		"none in the process items",
		"process: id string x3; idProcedimento string x3; usuarioAtribuido null x2, object x1; total string",
		"empty page: data key absent, total string",
		"activities: newest first (ids going down)",
		"documents: oldest first (ids going up)",
	}
	for _, line := range want {
		if !strings.Contains(stdout, line) {
			t.Errorf("output does not contain %q:\n%s", line, stdout)
		}
	}
	values := []string{
		"00000.", "9001", "50001", "80001", "SEDEMO", "fulano", "Fulano", "beltrano", "Beltrano",
		"Diárias", "Brasília", "Contrato", "Ofício", "Despacho", "recebido", "0012345", "06/11/2026", seitest.Password,
	}
	for _, value := range values {
		if strings.Contains(stdout, value) {
			t.Errorf("the output shows the value %q:\n%s", value, stdout)
		}
	}
}
