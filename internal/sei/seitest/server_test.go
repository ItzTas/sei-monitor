package seitest_test

import (
	"encoding/json"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
)

func TestFakeItemsUseOnlyFieldsOfTheRealFormat(t *testing.T) {
	server := seitest.New(t)
	server.SetProcesses(seitest.UnitNUPROC, seitest.Process{
		ID: 9001, Number: seitest.ProcessNumber(1), Type: "Contrato", ReturnDate: "06/11/2026",
		Assignee: &seitest.User{ID: seitest.UserID, Acronym: seitest.Username, Name: seitest.UserName},
	})
	server.AddActivities(9001, seitest.Activity{ID: 50001, User: seitest.Username, Unit: "SEDEMO/NUPROC", Text: "Processo recebido na unidade", At: time.Now()})
	server.AddDocuments(9001, seitest.Document{ID: 80001, Protocol: "0012345", Type: "Ofício", Number: "12", Label: "Ofício 12 (0012345)"})
	token := login(t, server)

	tests := []struct {
		path    string
		fixture string
	}{
		{"/processo/listar", "processo_listar.json"},
		{"/atividade/listar?procedimento=9001", "atividade_listar.json"},
		{"/documento/listar/9001", "documento_listar.json"},
		{"/versao", "versao.json"},
	}
	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			fake := fieldTypes(t, get(t, server, tt.path, token))
			real := fieldTypes(t, readFixture(t, tt.fixture))
			for field, kinds := range fake {
				for kind := range kinds {
					if !real[field][kind] && kind != "null" && !onlyNull(real[field]) {
						t.Errorf("fake field %s (%s) does not appear with that type in the fixture (%v)", field, kind, real[field])
					}
				}
			}
		})
	}
}

func login(t *testing.T, server *seitest.Server) string {
	t.Helper()
	form := url.Values{"usuario": {seitest.Username}, "senha": {seitest.Password}, "orgao": {"63"}}
	req, err := http.NewRequestWithContext(t.Context(), http.MethodPost, server.APIURL()+seitest.RouteLogin, strings.NewReader(form.Encode()))
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	var body struct {
		Data struct {
			Token string `json:"token"`
		} `json:"data"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&body); err != nil || body.Data.Token == "" {
		t.Fatalf("log in to the fake: %v", err)
	}
	return body.Data.Token
}

func get(t *testing.T, server *seitest.Server, path, token string) []byte {
	t.Helper()
	req, err := http.NewRequestWithContext(t.Context(), http.MethodGet, server.APIURL()+path, nil)
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("token", token)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = resp.Body.Close() }()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatal(err)
	}
	return body
}

func readFixture(t *testing.T, name string) []byte {
	t.Helper()
	body, err := os.ReadFile(filepath.Join("..", "..", "..", "testdata", "sei", name))
	if err != nil {
		t.Fatal(err)
	}
	return body
}

func fieldTypes(t *testing.T, body []byte) map[string]map[string]bool {
	t.Helper()
	var value any
	if err := json.Unmarshal(body, &value); err != nil {
		t.Fatalf("invalid JSON: %v", err)
	}
	types := map[string]map[string]bool{}
	collect("", value, types)
	return types
}

func collect(prefix string, value any, types map[string]map[string]bool) {
	if types[prefix] == nil {
		types[prefix] = map[string]bool{}
	}
	types[prefix][kind(value)] = true
	switch typed := value.(type) {
	case map[string]any:
		for key, item := range typed {
			collect(prefix+"."+key, item, types)
		}
	case []any:
		for _, item := range typed {
			collect(prefix+"[]", item, types)
		}
	}
}

func kind(value any) string {
	switch value.(type) {
	case map[string]any:
		return "object"
	case []any:
		return "list"
	case string:
		return "string"
	case nil:
		return "null"
	}
	return "number or boolean"
}

func onlyNull(kinds map[string]bool) bool {
	return len(kinds) == 1 && kinds["null"]
}
