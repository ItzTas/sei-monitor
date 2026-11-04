package sei_test

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/sei"
)

const apiPath = "/sei/modulos/wssei/controlador_ws.php/api/v2"

type fixture struct {
	status      int
	file        string
	contentType string
}

func fixtureServer(t *testing.T, routes map[string]fixture) *httptest.Server {
	t.Helper()
	mux := http.NewServeMux()
	for pattern, fx := range routes {
		body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "sei", fx.file))
		if err != nil {
			t.Fatalf("read fixture %s: %v", fx.file, err)
		}
		mux.HandleFunc(pattern, func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Type", cmpOr(fx.contentType, "application/json"))
			w.WriteHeader(cmpOr(fx.status, http.StatusOK))
			_, _ = w.Write(body)
		})
	}
	server := httptest.NewServer(mux)
	t.Cleanup(server.Close)
	return server
}

func cmpOr[T comparable](value, fallback T) T {
	var zero T
	if value == zero {
		return fallback
	}
	return value
}

func fixtureClient(t *testing.T, routes map[string]fixture) *sei.Client {
	t.Helper()
	routes["POST "+apiPath+"/autenticar"] = cmpOr(routes["POST "+apiPath+"/autenticar"], fixture{file: "autenticar.json"})
	server := fixtureServer(t, routes)
	location, err := time.LoadLocation("America/Boa_Vista")
	if err != nil {
		t.Fatal(err)
	}
	return sei.New(server.URL+apiPath, sei.Credentials{Username: "fulano.tal", Password: config.StaticPassword(config.NewSecret("x")), OrgID: 63}, sei.Options{UserAgent: "sei-monitor/teste", Location: location})
}

func TestProcessPageReadsTheModuleFormat(t *testing.T) {
	client := fixtureClient(t, map[string]fixture{"GET " + apiPath + "/processo/listar": {file: "processo_listar.json"}})

	page, err := client.ProcessPage(t.Context(), 1001, sei.ProcessQuery{Limit: 50})
	if err != nil {
		t.Fatal(err)
	}

	if page.Total != 3 || len(page.Items) != 3 {
		t.Fatalf("total = %d, items = %d; want 3 and 3", page.Total, len(page.Items))
	}
	assigned, unassigned, overdue := page.Items[0], page.Items[1], page.Items[2]

	if assigned.ProcedureID != 9001 || assigned.Number != "00000.000001/2026-00" || assigned.Origin != "R" {
		t.Errorf("assigned process = %+v", assigned)
	}
	if assigned.Type != "Administração: Diárias" || assigned.Description != `Pedido de diárias "viagem"` {
		t.Errorf("type and description with unresolved HTML entities: %q, %q", assigned.Type, assigned.Description)
	}
	if assigned.Assignee == nil || *assigned.Assignee != (sei.User{ID: 101, Acronym: "fulano.tal", Name: "Fulano de Tal"}) {
		t.Errorf("assignee = %+v", assigned.Assignee)
	}
	if want := time.Date(2026, 11, 6, 0, 0, 0, 0, time.UTC); !assigned.ReturnDate.Equal(want) || assigned.ReturnOverdue {
		t.Errorf("return = %v overdue=%v; want %v, not overdue", assigned.ReturnDate, assigned.ReturnOverdue, want)
	}

	if unassigned.Assignee != nil || !unassigned.ReturnDate.IsZero() || unassigned.Origin != "G" {
		t.Errorf("process without assignee or scheduled return = %+v", unassigned)
	}

	if overdue.ProcedureID != 9003 || !overdue.ReturnOverdue || !overdue.ReturnDate.Equal(time.Date(2026, 10, 30, 0, 0, 0, 0, time.UTC)) {
		t.Errorf("process with an overdue return and numeric ids = %+v", overdue)
	}
}

func TestActivityPageConvertsTheLocalTimeToUTC(t *testing.T) {
	client := fixtureClient(t, map[string]fixture{"GET " + apiPath + "/atividade/listar": {file: "atividade_listar.json"}})

	page, err := client.ActivityPage(t.Context(), 1001, 9001, 0, 20)
	if err != nil {
		t.Fatal(err)
	}

	want := []sei.Activity{
		{ID: 50003, ProcedureID: 9001, User: "fulano.tal", Unit: "SEDEMO/NUPROC", Text: "Processo recebido na unidade", At: time.Date(2026, 11, 4, 17, 13, 5, 0, time.UTC)},
		{ID: 50002, ProcedureID: 9001, Unit: "SEDEMO/PROTOCOLO", Text: "Processo remetido pela unidade SEDEMO/PROTOCOLO", At: time.Date(2026, 11, 3, 13, 0, 0, 0, time.UTC)},
		{ID: 50001, ProcedureID: 9001, User: "beltrano", Unit: "SEDEMO/PROTOCOLO", Text: `Documento "Ofício" assinado`, At: time.Date(2026, 11, 1, 21, 45, 30, 0, time.UTC)},
	}
	if len(page.Items) != len(want) {
		t.Fatalf("activities = %d, want %d", len(page.Items), len(want))
	}
	for i := range want {
		if page.Items[i] != want[i] {
			t.Errorf("activity %d = %+v\nwant %+v", i, page.Items[i], want[i])
		}
	}
}

func TestDocumentPageNamesEachDocument(t *testing.T) {
	client := fixtureClient(t, map[string]fixture{"GET " + apiPath + "/documento/listar/9001": {file: "documento_listar.json"}})

	page, err := client.DocumentPage(t.Context(), 1001, 9001, 0, 50)
	if err != nil {
		t.Fatal(err)
	}

	want := []sei.Document{
		{ID: 80001, ProcedureID: 9001, Protocol: "0012345", Type: "Ofício", Name: "Ofício 12 (0012345)"},
		{ID: 80002, ProcedureID: 9001, Protocol: "0012346", Type: "Anexo", Name: "Anexo"},
	}
	if len(page.Items) != len(want) || page.Total != 2 {
		t.Fatalf("documents = %+v (total %d)", page.Items, page.Total)
	}
	for i := range want {
		if page.Items[i] != want[i] {
			t.Errorf("document %d = %+v\nwant %+v", i, page.Items[i], want[i])
		}
	}
}

func TestEmptyListWithoutDataKey(t *testing.T) {
	client := fixtureClient(t, map[string]fixture{"GET " + apiPath + "/processo/listar": {file: "lista_vazia.json"}})

	processes, err := client.Processes(t.Context(), 1001)
	if err != nil {
		t.Fatal(err)
	}
	if len(processes) != 0 {
		t.Fatalf("processes = %+v, want none", processes)
	}
}

func TestAPIErrorsKeepOnlyTheMessage(t *testing.T) {
	tests := []struct {
		name    string
		file    string
		message string
	}{
		{"HTML entities resolved and exception dropped", "erro_entidade.json", "O processo n'foi encontrado na unidade."},
		{"message list", "erro_lista.json", "Informe o limite.; Informe a página."},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			client := fixtureClient(t, map[string]fixture{"GET " + apiPath + "/processo/listar": {file: tt.file}})

			_, err := client.ProcessPage(t.Context(), 1001, sei.ProcessQuery{Limit: 50})

			apiErr, ok := errors.AsType[*sei.APIError](err)
			if !ok {
				t.Fatalf("error = %v, want *sei.APIError", err)
			}
			if apiErr.Message != tt.message {
				t.Errorf("message = %q, want %q", apiErr.Message, tt.message)
			}
			for _, leak := range []string{"SELECT", "MdWsSei", "Exceção salva"} {
				if strings.Contains(err.Error(), leak) {
					t.Errorf("the error %q carries the exception field (%q)", err, leak)
				}
			}
		})
	}
}

func TestLoginFailures(t *testing.T) {
	tests := []struct {
		name    string
		login   fixture
		want    error
		notWant error
	}{
		{"rejected credentials", fixture{file: "login_recusado.json"}, sei.ErrInvalidCredentials, sei.ErrUnauthorized},
		{"disabled module is not a credential problem", fixture{status: http.StatusUnauthorized, file: "modulo_inativo.json"}, sei.ErrModuleUnavailable, sei.ErrInvalidCredentials},
		{"HTML page instead of the API", fixture{status: http.StatusOK, file: "login.html", contentType: "text/html"}, sei.ErrUnexpectedResponse, sei.ErrInvalidCredentials},
		{"internal error is transient", fixture{status: http.StatusBadGateway, file: "login.html", contentType: "text/html"}, sei.ErrServer, sei.ErrInvalidCredentials},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			client := fixtureClient(t, map[string]fixture{"POST " + apiPath + "/autenticar": tt.login})

			_, err := client.Login(t.Context())

			if !errors.Is(err, tt.want) {
				t.Errorf("error = %v, want %v", err, tt.want)
			}
			if errors.Is(err, tt.notWant) {
				t.Errorf("error = %v must not be %v", err, tt.notWant)
			}
		})
	}
}

func TestListOrgsReadsTheLatin1LoginPageAfterRedirect(t *testing.T) {
	server := fixtureServer(t, map[string]fixture{
		"GET /sip/login.php": {file: "login.html", contentType: "text/html; charset=iso-8859-1"},
	})
	mux := http.NewServeMux()
	mux.HandleFunc("GET /sei/{$}", func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, server.URL+"/sip/login.php?sigla_orgao_sistema=SEDEMO&sigla_sistema=SEI", http.StatusFound)
	})
	origin := httptest.NewServer(mux)
	t.Cleanup(origin.Close)

	orgs, err := sei.ListOrgs(t.Context(), origin.URL, sei.Options{UserAgent: "sei-monitor/teste"})
	if err != nil {
		t.Fatal(err)
	}

	want := []sei.Org{{ID: 34, Acronym: "ADEMO"}, {ID: 48, Acronym: "BDEMO"}, {ID: 12, Acronym: "CDEMO"}, {ID: 63, Acronym: "SEDEMO"}, {ID: 77, Acronym: "ÓRGÃODEMO"}}
	if len(orgs) != len(want) {
		t.Fatalf("orgs = %+v, want %+v", orgs, want)
	}
	for i := range want {
		if orgs[i] != want[i] {
			t.Errorf("org %d = %+v, want %+v", i, orgs[i], want[i])
		}
	}
}

func TestAPICallsDoNotFollowRedirects(t *testing.T) {
	var leaked bool
	elsewhere := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { leaked = true }))
	t.Cleanup(elsewhere.Close)
	mux := http.NewServeMux()
	mux.HandleFunc("POST "+apiPath+"/autenticar", func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, elsewhere.URL+"/autenticar", http.StatusTemporaryRedirect)
	})
	origin := httptest.NewServer(mux)
	t.Cleanup(origin.Close)
	client := sei.New(origin.URL+apiPath, sei.Credentials{Username: "fulano.tal", Password: config.StaticPassword(config.NewSecret("p4ss")), OrgID: 63}, sei.Options{})

	_, err := client.Login(t.Context())

	if !errors.Is(err, sei.ErrRedirect) {
		t.Fatalf("error = %v, want sei.ErrRedirect", err)
	}
	if leaked {
		t.Fatal("the login was sent to the redirect target")
	}
}

func TestDiscover(t *testing.T) {
	tests := []struct {
		name     string
		routes   map[string]fixture
		wantPath string
		wantErr  error
	}{
		{
			name:     "module under /wssei answers 401 without a token",
			routes:   map[string]fixture{"GET /sei/modulos/wssei/controlador_ws.php/api/v2/versao": {status: http.StatusUnauthorized, file: "acesso_negado.json"}},
			wantPath: "/sei/modulos/wssei/controlador_ws.php/api/v2",
		},
		{
			name:     "module under /mod-wssei",
			routes:   map[string]fixture{"GET /sei/modulos/mod-wssei/controlador_ws.php/api/v2/versao": {status: http.StatusUnauthorized, file: "acesso_negado.json"}},
			wantPath: "/sei/modulos/mod-wssei/controlador_ws.php/api/v2",
		},
		{
			name:     "disabled module",
			routes:   map[string]fixture{"GET /sei/modulos/wssei/controlador_ws.php/api/v2/versao": {status: http.StatusUnauthorized, file: "modulo_inativo.json"}},
			wantPath: "/sei/modulos/wssei/controlador_ws.php/api/v2",
			wantErr:  sei.ErrModuleUnavailable,
		},
		{
			name:    "no module",
			routes:  map[string]fixture{"GET /sei/{$}": {file: "login.html", contentType: "text/html"}},
			wantErr: sei.ErrModuleNotFound,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server := fixtureServer(t, tt.routes)

			base, err := sei.Discover(t.Context(), server.URL, sei.Options{})

			if !errors.Is(err, tt.wantErr) {
				t.Fatalf("error = %v, want %v", err, tt.wantErr)
			}
			if tt.wantPath != "" && base != server.URL+tt.wantPath {
				t.Errorf("base = %q, want %q", base, server.URL+tt.wantPath)
			}
		})
	}
}
