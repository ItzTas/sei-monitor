package sei

import (
	"context"
	"errors"
	"go/ast"
	"go/parser"
	"go/token"
	"html"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/goldentest"
)

func TestGoldenVectors(t *testing.T) {
	goldentest.Golden(t, "fold.json", foldVectors())
	goldentest.Golden(t, "classify.json", classifyVectors(t))
	goldentest.Golden(t, "html_unescape.json", unescapeVectors())
	goldentest.Golden(t, "html_entities.json", entityVectors(t))
	goldentest.Golden(t, "wire.json", wireVectors(t))
	goldentest.Golden(t, "orgs_page.json", orgsPageVectors(t))
}

var goldenErrorKinds = []struct {
	err  error
	name string
}{
	{ErrModuleUnavailable, "module_unavailable"},
	{ErrServer, "server"},
	{ErrInvalidCredentials, "invalid_credentials"},
	{ErrUnauthorized, "unauthorized"},
	{ErrRefused, "refused"},
	{ErrRedirect, "redirect"},
	{ErrUnexpectedResponse, "unexpected_response"},
}

func sentinelName(err error) (string, bool) {
	for _, kind := range goldenErrorKinds {
		if errors.Is(err, kind.err) {
			return kind.name, true
		}
	}
	return "", false
}

type goldenText struct {
	Input  string `json:"input"`
	Output string `json:"output"`
}

func goldenTexts(inputs []string, transform func(string) string) []goldenText {
	cases := make([]goldenText, len(inputs))
	for i, input := range inputs {
		cases[i] = goldenText{Input: input, Output: transform(input)}
	}
	return cases
}

func foldVectors() []goldenText {
	return goldenTexts([]string{
		"Módulo inativo.", "MÓDULO INCOMPATÍVEL com a versão", "  Token inválido! ", "TOKEN",
		"Usuário ou Senha Inválida.", "e\u0301", "\ufb01", "\u0130stanbul", "\u0391\u03a3", "ß", "\u01c5", "",
		"   ", "\u00a0Módulo inativo.\u3000", "\u0301", "\u03a9", "\u2126", "Å", "\u212b", "\ud55c", "\u1ec6", "a\u0308\u0301",
		"\u0915\u093e", "\u1fb3", "ÇÃO çã ÕÑ", "\u01c4", "\u1e9e", "Tôken", "ção \u200b token", "\tMÓDULO\n",
		"\u0085x\u0085", "x\u200b", "\uff21", "\u216b", "\U0001d400",
	}, fold)
}

type classifyCase struct {
	Route   string `json:"route"`
	Status  int    `json:"status"`
	Message string `json:"message"`
	Kind    string `json:"kind"`
}

func classifyVectors(t *testing.T) []classifyCase {
	cases := []classifyCase{
		{Route: routeProcesses, Status: 500, Message: "Módulo inativo."},
		{Route: routeLogin, Status: 401, Message: "Módulo inativo."},
		{Route: routeVersion, Status: 200, Message: "MÓDULO INCOMPATÍVEL com a versão"},
		{Route: routeProcesses, Status: 200, Message: "  módulo inativo"},
		{Route: routeProcesses, Status: 200, Message: "O módulo inativo"},
		{Route: routeProcesses, Status: 200, Message: "Modulo inativo."},
		{Route: routeLogin, Status: 500, Message: "Usuário ou Senha Inválida."},
		{Route: routeLogin, Status: 502, Message: ""},
		{Route: routeProcesses, Status: 503, Message: "Token inválido!"},
		{Route: routeProcesses, Status: 599, Message: ""},
		{Route: routeProcesses, Status: 600, Message: ""},
		{Route: routeLogin, Status: 401, Message: ""},
		{Route: routeLogin, Status: 403, Message: "Acesso negado!"},
		{Route: routeLogin, Status: 200, Message: "Token inválido!"},
		{Route: routeLogin, Status: 200, Message: "Usuário ou Senha Inválida."},
		{Route: routeProcesses, Status: 200, Message: "O token informado expirou"},
		{Route: routeProcesses, Status: 200, Message: "TOKEN"},
		{Route: routeProcesses, Status: 200, Message: "Tôken expirado"},
		{Route: routeProcesses, Status: 200, Message: "ação tokenização"},
		{Route: routeProcesses, Status: 401, Message: ""},
		{Route: routeProcesses, Status: 403, Message: "Acesso negado!"},
		{Route: routeProcesses, Status: 200, Message: "Unidade não permitida para o usuário."},
		{Route: routeProcesses, Status: 400, Message: ""},
		{Route: routeProcesses, Status: 404, Message: "Não encontrado."},
		{Route: routeProcesses, Status: 499, Message: ""},
		{Route: routeActivities, Status: 200, Message: "Processo não encontrado."},
		{Route: routeDocuments, Status: 302, Message: ""},
		{Route: routeVersion, Status: 401, Message: "Acesso negado!"},
		{Route: "/AUTENTICAR", Status: 200, Message: "Usuário ou Senha Inválida."},
		{Route: routeProcesses, Status: 200, Message: "to ken"},
	}
	for i := range cases {
		name, ok := sentinelName(classify(cases[i].Route, cases[i].Status, cases[i].Message))
		if !ok {
			t.Fatalf("classify(%+v) returned an unknown kind", cases[i])
		}
		cases[i].Kind = name
	}
	return cases
}

func unescapeVectors() []goldenText {
	return goldenTexts([]string{
		"&amp;amp;", "&#8211", "&#0;", "&#128;", "&#xD800;", "&#1114112;", "&notit;", "&amp", "&ampx", "&nbsp;",
		"&unknown;", "&", "&;", "&#;", "&#x;", "Administra&ccedil;&atilde;o", "&#039;", "&quot;", "&ccedil",
		"&AMP;", "&notin;", "&NotNestedGreaterGreater;", "&#x1F600;", "&#9999999999;", "&#xFFFFFFFF;",
		"&#x110000;", "&#x9F;", "&#x80;", "&#x81;", "&#13;", "&#65", "&#65x", "&#X41;", "&#x41", "&#x", "&#",
		"&#1", "&ampamp;", "&notin", "&lt", "&LT", "&Lt;", "&&amp;", "a&b", "ção &amp; ção", "ãã&ccedil",
		"\u20ac&eacute;\u20ac", "&aaaaaaaaaaaaaaaaaaaa", "&aacute&eacute;", "&#x2013;&ndash;&#8211;", "x&", "&#00065;",
		"&#x0000041;", "&nbsp", "&nbspx", "&copy2026", "&ThickSpace;", "&nGt;", "&ngE;", "",
	}, html.UnescapeString)
}

func entityVectors(t *testing.T) map[string]string {
	version, err := exec.CommandContext(t.Context(), "go", "env", "GOVERSION").Output()
	if err != nil {
		t.Fatal(err)
	}
	if strings.TrimSpace(string(version)) != runtime.Version() {
		t.Fatalf("go env GOVERSION = %s, but the test binary was built with %s", version, runtime.Version())
	}
	root, err := exec.CommandContext(t.Context(), "go", "env", "GOROOT").Output()
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(strings.TrimSpace(string(root)), "src", "html", "entity.go")
	file, err := parser.ParseFile(token.NewFileSet(), path, nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	entities := map[string]string{}
	ast.Inspect(file, func(node ast.Node) bool {
		literal, ok := node.(*ast.CompositeLit)
		if !ok {
			return true
		}
		if _, isMap := literal.Type.(*ast.MapType); !isMap {
			return true
		}
		for _, element := range literal.Elts {
			pair := element.(*ast.KeyValueExpr)
			name, err := strconv.Unquote(pair.Key.(*ast.BasicLit).Value)
			if err != nil {
				t.Fatal(err)
			}
			want := entityValue(t, pair.Value)
			got := html.UnescapeString("&" + name)
			if got != want {
				t.Fatalf("html.UnescapeString(%q) = %q, the table says %q", "&"+name, got, want)
			}
			entities[name] = got
		}
		return false
	})
	if len(entities) < 2000 {
		t.Fatalf("read %d entities from %s", len(entities), path)
	}
	return entities
}

func entityValue(t *testing.T, value ast.Expr) string {
	t.Helper()
	runes := []ast.Expr{value}
	if pair, ok := value.(*ast.CompositeLit); ok {
		runes = pair.Elts
	}
	var text strings.Builder
	for _, expr := range runes {
		char, err := strconv.Unquote(expr.(*ast.BasicLit).Value)
		if err != nil {
			t.Fatal(err)
		}
		text.WriteString(char)
	}
	return text.String()
}

type goldenError struct {
	Kind    string `json:"kind"`
	Message string `json:"message"`
}

func describeError(t *testing.T, err error) *goldenError {
	t.Helper()
	if err == nil {
		return nil
	}
	text := err.Error()
	if name, ok := sentinelName(err); ok {
		return &goldenError{Kind: name, Message: text}
	}
	if strings.HasPrefix(text, "sei: read the ") {
		prefix, _, found := strings.Cut(text, " answer: ")
		if !found {
			t.Fatalf("decode error without the answer prefix: %q", text)
		}
		return &goldenError{Kind: "decode", Message: prefix + " answer: "}
	}
	if strings.HasSuffix(text, "SEI did not return a session token") {
		return &goldenError{Kind: "no_token", Message: text}
	}
	t.Fatalf("unclassified error %q", text)
	return nil
}

type goldenPage[T any] struct {
	Items []T `json:"items"`
	Total int `json:"total"`
}

type goldenUser struct {
	ID      int64  `json:"id"`
	Acronym string `json:"acronym"`
	Name    string `json:"name"`
}

type goldenProcess struct {
	ProcedureID   int64       `json:"procedure_id"`
	Number        string      `json:"number"`
	Type          string      `json:"type"`
	Description   string      `json:"description"`
	Assignee      *goldenUser `json:"assignee"`
	ReturnDate    string      `json:"return_date"`
	ReturnOverdue bool        `json:"return_overdue"`
	Origin        string      `json:"origin"`
}

type goldenActivity struct {
	ID          int64  `json:"id"`
	ProcedureID int64  `json:"procedure_id"`
	User        string `json:"user"`
	Unit        string `json:"unit"`
	Text        string `json:"text"`
	At          string `json:"at"`
}

type goldenDocument struct {
	ID          int64  `json:"id"`
	ProcedureID int64  `json:"procedure_id"`
	Protocol    string `json:"protocol"`
	Type        string `json:"type"`
	Name        string `json:"name"`
}

type goldenVersion struct {
	SEI    string `json:"sei"`
	Module string `json:"module"`
}

type goldenUnit struct {
	ID          int64  `json:"id"`
	Acronym     string `json:"acronym"`
	Description string `json:"description"`
}

type goldenSession struct {
	UserID        int64        `json:"user_id"`
	UserAcronym   string       `json:"user_acronym"`
	UserName      string       `json:"user_name"`
	CurrentUnitID int64        `json:"current_unit_id"`
	Units         []goldenUnit `json:"units"`
}

func convertPage[T, R any](page Page[T], convert func(T) R) goldenPage[R] {
	items := make([]R, len(page.Items))
	for i, item := range page.Items {
		items[i] = convert(item)
	}
	return goldenPage[R]{Items: items, Total: page.Total}
}

func processRecord(process Process) goldenProcess {
	record := goldenProcess{
		ProcedureID:   process.ProcedureID,
		Number:        process.Number,
		Type:          process.Type,
		Description:   process.Description,
		ReturnOverdue: process.ReturnOverdue,
		Origin:        process.Origin,
	}
	if process.Assignee != nil {
		record.Assignee = &goldenUser{ID: process.Assignee.ID, Acronym: process.Assignee.Acronym, Name: process.Assignee.Name}
	}
	if !process.ReturnDate.IsZero() {
		record.ReturnDate = process.ReturnDate.Format(time.DateOnly)
	}
	return record
}

func activityRecord(activity Activity) goldenActivity {
	return goldenActivity{
		ID:          activity.ID,
		ProcedureID: activity.ProcedureID,
		User:        activity.User,
		Unit:        activity.Unit,
		Text:        activity.Text,
		At:          activity.At.UTC().Format(time.RFC3339Nano),
	}
}

func documentRecord(document Document) goldenDocument {
	return goldenDocument(document)
}

func sessionRecord(session Session) goldenSession {
	units := make([]goldenUnit, len(session.Units))
	for i, unit := range session.Units {
		units[i] = goldenUnit(unit)
	}
	return goldenSession{
		UserID:        session.UserID,
		UserAcronym:   session.UserAcronym,
		UserName:      session.UserName,
		CurrentUnitID: session.CurrentUnitID,
		Units:         units,
	}
}

type wireInput struct {
	name   string
	route  string
	zone   string
	status int
	body   []byte
}

type wireCase struct {
	Name   string       `json:"name"`
	Route  string       `json:"route"`
	Zone   string       `json:"zone"`
	Status int          `json:"status"`
	Body   []byte       `json:"body_base64"`
	Result any          `json:"result"`
	Error  *goldenError `json:"error"`
}

func wireVectors(t *testing.T) []wireCase {
	inputs := wireInputs(t)
	cases := make([]wireCase, len(inputs))
	for i, input := range inputs {
		cases[i] = runWireCase(t, input)
	}
	return cases
}

func runWireCase(t *testing.T, input wireInput) wireCase {
	t.Helper()
	patterns := map[string]string{
		"processes":  "GET " + routeProcesses,
		"activities": "GET " + routeActivities,
		"documents":  "GET " + routeDocuments + "/9001",
		"version":    "GET " + routeVersion,
		"login":      "POST " + routeLogin,
	}
	pattern, ok := patterns[input.route]
	if !ok {
		t.Fatalf("unknown route %q", input.route)
	}
	mux := http.NewServeMux()
	if input.route != "login" {
		mux.HandleFunc("POST "+routeLogin, func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte(`{"sucesso":true,"data":{"token":"t"}}`))
		})
	}
	mux.HandleFunc(pattern, func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(input.status)
		_, _ = w.Write(input.body)
	})
	server := httptest.NewServer(mux)
	defer server.Close()

	location, err := time.LoadLocation(input.zone)
	if err != nil {
		t.Fatal(err)
	}
	creds := Credentials{Username: "user.name", Password: core.StaticPassword(core.NewSecret("s3cr3t-password")), OrgID: 63}
	client := New(server.URL, creds, Options{UserAgent: "sei-monitor/golden", Location: location})
	result, err := wireCall(t.Context(), client, input.route)
	record := wireCase{Name: input.name, Route: input.route, Zone: input.zone, Status: input.status, Body: input.body, Error: describeError(t, err)}
	if err == nil {
		record.Result = result
	}
	return record
}

func wireCall(ctx context.Context, client *Client, route string) (any, error) {
	calls := map[string]func() (any, error){
		"processes": func() (any, error) {
			page, err := client.ProcessPage(ctx, 1001, ProcessQuery{Page: 0, Limit: 50})
			return convertPage(page, processRecord), err
		},
		"activities": func() (any, error) {
			page, err := client.ActivityPage(ctx, 1001, 9001, 0, 20)
			return convertPage(page, activityRecord), err
		},
		"documents": func() (any, error) {
			page, err := client.DocumentPage(ctx, 1001, 9001, 0, 50)
			return convertPage(page, documentRecord), err
		},
		"version": func() (any, error) {
			version, err := client.Version(ctx)
			return goldenVersion(version), err
		},
		"login": func() (any, error) {
			session, err := client.Login(ctx)
			return sessionRecord(session), err
		},
	}
	return calls[route]()
}

func fixture(t *testing.T, name string) []byte {
	t.Helper()
	body, err := os.ReadFile(filepath.Join("..", "..", "testdata", "sei", name))
	if err != nil {
		t.Fatal(err)
	}
	return body
}

func pageBody(items ...string) []byte {
	return []byte(`{"sucesso":true,"data":[` + strings.Join(items, ",") + `],"total":"` + strconv.Itoa(len(items)) + `"}`)
}

func item(id, attributes string) string {
	return `{"id":` + id + `,"atributos":{` + attributes + `}}`
}

func wireInputs(t *testing.T) []wireInput {
	var inputs []wireInput
	add := func(name, route string, status int, body []byte) {
		inputs = append(inputs, wireInput{name: name, route: route, zone: "America/Boa_Vista", status: status, body: body})
	}

	fixtures := []struct {
		file   string
		route  string
		status int
	}{
		{"processo_listar.json", "processes", 200},
		{"lista_vazia.json", "processes", 200},
		{"erro_entidade.json", "processes", 200},
		{"erro_lista.json", "processes", 200},
		{"token_invalido.json", "processes", 401},
		{"acesso_negado.json", "processes", 403},
		{"modulo_inativo.json", "processes", 200},
		{"login.html", "processes", 200},
		{"atividade_listar.json", "activities", 200},
		{"documento_listar.json", "documents", 200},
		{"versao.json", "version", 200},
		{"acesso_negado.json", "version", 401},
		{"autenticar.json", "login", 200},
		{"login_recusado.json", "login", 200},
		{"modulo_inativo.json", "login", 401},
		{"login.html", "login", 200},
		{"login.html", "login", 502},
		{"token_invalido.json", "login", 401},
	}
	for _, fx := range fixtures {
		add("fixture "+fx.file+" on "+fx.route+" with HTTP "+strconv.Itoa(fx.status), fx.route, fx.status, fixture(t, fx.file))
	}

	ids := []string{
		`9001`, `"9001"`, `""`, `null`, `" 7 "`, `"12.0"`, `"12.5"`, `"1e3"`, `"abc"`, `9.3e18`, `9.2e18`, `-9.3e18`,
		`"9223372036854775807"`, `"-9223372036854775809"`, `true`, `[1]`, `{}`, `"0x1p4"`,
		`"NaN"`, `"Infinity"`, `"+5"`, `"12"`, `"1_000"`, `12.0`, `1e3`, `-0`, `"-7"`, `" "`,
	}
	for _, id := range ids {
		add("process id "+id, "processes", 200, pageBody(item(id, `"numero":"00000.000001/2026-00"`)))
	}
	add("idProcedimento wins over id", "processes", 200, pageBody(item(`"9001"`, `"idProcedimento":"9002"`)))
	add("idProcedimento zero falls back to id", "processes", 200, pageBody(item(`"9001"`, `"idProcedimento":"0"`)))
	add("idProcedimento invalid", "processes", 200, pageBody(item(`"9001"`, `"idProcedimento":"x"`)))
	add("idProcedimento and id both missing", "processes", 200, pageBody(`{"atributos":{"numero":"1"}}`))
	add("process without atributos", "processes", 200, pageBody(`{"id":"9001"}`))

	for _, total := range []string{`"3"`, `3`, `"abc"`, `"12.5"`, `null`, `" 4 "`, `2.0`, `""`, `-1`, `"1e2"`, `true`} {
		add("total "+total, "processes", 200, []byte(`{"sucesso":true,"data":[],"total":`+total+`}`))
	}
	add("total missing", "processes", 200, []byte(`{"sucesso":true,"data":[]}`))

	for _, data := range []string{`null`, `false`, `""`, `[]`, `{}`, `0`, `true`, `" "`, `"x"`, `[null]`, `[[]]`, `[1]`} {
		add("data "+data, "processes", 200, []byte(`{"sucesso":true,"data":`+data+`,"total":"0"}`))
	}

	envelopes := []struct {
		name   string
		status int
		body   string
	}{
		{"sucesso missing", 200, `{"data":[]}`},
		{"sucesso as text", 200, `{"sucesso":"true","data":[]}`},
		{"sucesso null", 200, `{"sucesso":null,"data":[]}`},
		{"sucesso as number", 200, `{"sucesso":1,"data":[]}`},
		{"sucesso false with 200", 200, `{"sucesso":false,"mensagem":"Falha."}`},
		{"sucesso false without mensagem", 200, `{"sucesso":false}`},
		{"upper-case keys", 200, `{"SUCESSO":true,"DATA":[{"ID":"9001","Atributos":{"IdProcedimento":"9005","NUMERO":"1"}}],"Total":"1"}`},
		{"repeated key, last wins", 200, `{"sucesso":false,"sucesso":true,"data":[],"total":"1","total":"2"}`},
		{"repeated key in another case, last wins", 200, `{"sucesso":true,"SUCESSO":false,"mensagem":"Falha."}`},
		{"empty object", 200, `{}`},
		{"empty body", 200, ``},
		{"JSON array", 200, `[]`},
		{"trailing garbage", 200, `{"sucesso":true,"data":[]} x`},
		{"redirect", 302, `{"sucesso":true,"data":[]}`},
		{"401 with envelope", 401, `{"sucesso":false,"mensagem":"Token inválido!"}`},
		{"401 without envelope", 401, `Unauthorized`},
		{"401 with sucesso true", 401, `{"sucesso":true,"data":[]}`},
		{"403 with envelope", 403, `{"sucesso":false,"mensagem":"Acesso negado!"}`},
		{"403 without envelope", 403, `<html>Forbidden</html>`},
		{"404 without envelope", 404, `Not Found`},
		{"500 with envelope", 500, `{"sucesso":false,"mensagem":"Erro interno."}`},
		{"500 with sucesso true", 500, `{"sucesso":true,"data":[]}`},
		{"500 without envelope", 500, `<html>Internal Server Error</html>`},
		{"502 with envelope", 502, `{"sucesso":false,"mensagem":"Bad gateway"}`},
		{"502 without envelope", 502, ``},
		{"204 without body", 204, ``},
		{"mensagem with entities and spaces", 200, `{"sucesso":false,"mensagem":"  Erro &amp; aviso&nbsp; "}`},
		{"mensagem list", 200, `{"sucesso":false,"mensagem":["  a &lt;b&gt; ","c&amp;",""]}`},
		{"mensagem number", 200, `{"sucesso":false,"mensagem":42}`},
		{"mensagem null", 200, `{"sucesso":false,"mensagem":null}`},
		{"mensagem empty list", 200, `{"sucesso":false,"mensagem":[]}`},
		{"mensagem list with a number", 200, `{"sucesso":false,"mensagem":["a",1]}`},
		{"mensagem object", 200, `{"sucesso":false,"mensagem":{"a":"b"}}`},
		{"mensagem with multi-byte text before an entity", 200, `{"sucesso":false,"mensagem":"ção &amp; ção &#039;"}`},
		{"mensagem token after entities", 200, `{"sucesso":false,"mensagem":"T&ocirc;ken expirado"}`},
		{"mensagem module after entities", 200, `{"sucesso":false,"mensagem":"M&oacute;dulo inativo."}`},
		{"mensagem module with 200 and sucesso false", 200, `{"sucesso":false,"mensagem":"Módulo inativo."}`},
		{"mensagem unit not allowed", 200, `{"sucesso":false,"mensagem":"Unidade não permitida para o usuário."}`},
		{"mensagem with invalid bytes", 200, `{"sucesso":false,"mensagem":"a` + "\xff" + `b` + "\xe2\x82" + `c"}`},
	}
	for _, envelope := range envelopes {
		add(envelope.name, "processes", envelope.status, []byte(envelope.body))
	}

	inputs = append(inputs, processInputs()...)
	inputs = append(inputs, activityInputs()...)
	inputs = append(inputs, documentInputs()...)

	versions := []struct {
		name string
		body string
	}{
		{"version data missing", `{"sucesso":true}`},
		{"version data null", `{"sucesso":true,"data":null}`},
		{"version data empty object", `{"sucesso":true,"data":{}}`},
		{"version with entities and spaces", `{"sucesso":true,"data":{"sei":" 5.0.5 &amp; ","wssei":"3.0.4&lt;"}}`},
		{"version number", `{"sucesso":true,"data":{"sei":"5.0.5","wssei":3}}`},
		{"version upper-case keys", `{"sucesso":true,"data":{"SEI":"5.0.5","WSSEI":"3.0.4"}}`},
		{"version data as list", `{"sucesso":true,"data":[]}`},
	}
	for _, version := range versions {
		add(version.name, "version", 200, []byte(version.body))
	}

	logins := []struct {
		name   string
		status int
		body   string
	}{
		{"login without token", 200, `{"sucesso":true,"data":{"loginData":{"IdUsuario":"101"}}}`},
		{"login with empty token", 200, `{"sucesso":true,"data":{"token":""}}`},
		{"login data null", 200, `{"sucesso":true,"data":null}`},
		{"login data missing", 200, `{"sucesso":true}`},
		{"login token as number", 200, `{"sucesso":true,"data":{"token":7}}`},
		{"login IdUsuario invalid", 200, `{"sucesso":true,"data":{"token":"t","loginData":{"IdUsuario":"abc"}}}`},
		{"login unidades null", 200, `{"sucesso":true,"data":{"token":"t","loginData":{"IdUsuario":101,"IdUnidadeAtual":"1001","sigla":"user.name","nome":"User Name"},"unidades":null}}`},
		{"login upper-case keys", 200, `{"sucesso":true,"data":{"TOKEN":"t","LOGINDATA":{"idusuario":"101","IDUNIDADEATUAL":"1001","SIGLA":"user.name","NOME":"User Name"},"UNIDADES":[{"ID":"1001","SIGLA":"SEDEMO/NUPROC","DESCRICAO":"Nucleo"}]}}`},
		{"login with entities and spaces everywhere", 200, `{"sucesso":true,"data":{"token":"t","loginData":{"IdUsuario":"101","IdUnidadeAtual":"1001","sigla":" user&amp;name ","nome":" User &amp; Name "},"unidades":[{"id":"1001","sigla":" SEDEMO&amp;NUPROC ","descricao":" N&uacute;cleo &amp; Gabinete "}]}}`},
		{"login redirect", 302, `{"sucesso":true,"data":{"token":"t"}}`},
		{"login 500 with envelope", 500, `{"sucesso":false,"mensagem":"Erro interno."}`},
		{"login 403 with envelope", 403, `{"sucesso":false,"mensagem":"Acesso negado!"}`},
		{"login 200 with sucesso false and no message", 200, `{"sucesso":false}`},
		{"login with invalid bytes", 200, `{"sucesso":true,"data":{"token":"t","loginData":{"nome":"a` + "\xff" + `b` + "\xe2\x82" + `c"}}}`},
	}
	for _, login := range logins {
		add(login.name, "login", login.status, []byte(login.body))
	}

	for _, route := range []string{"activities", "documents", "version"} {
		add("redirect on "+route, route, 302, []byte(``))
		add("500 without envelope on "+route, route, 500, []byte(`oops`))
	}
	inputs = append(inputs, wireInput{name: "process page in America/Sao_Paulo", route: "processes", zone: "America/Sao_Paulo", status: http.StatusOK, body: fixture(t, "processo_listar.json")})
	return inputs
}

func processInputs() []wireInput {
	cases := []struct {
		name  string
		items []string
	}{
		{"scheduled return carried from the previous item", []string{
			item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":{"dataProgramada":"06/11/2026"}}`),
			item(`"9002"`, `"status":{"retornoProgramado":"N","retornoData":{"dataProgramada":"06/11/2026"}}`),
		}},
		{"retornoData as text", []string{item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":"06/11/2026"}`)}},
		{"retornoData 31/02/2026", []string{item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":{"dataProgramada":"31/02/2026"}}`)}},
		{"retornoData 5/11/2026", []string{item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":"5/11/2026"}`)}},
		{"retornoData with spaces", []string{item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":{"dataProgramada":" 06/11/2026 "}}`)}},
		{"retornoData empty dataProgramada", []string{item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":{"dataProgramada":""}}`)}},
		{"retornoData null", []string{item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":null}`)}},
		{"retornoData number", []string{item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":6112026}`)}},
		{"retornoData dataProgramada number", []string{item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":{"dataProgramada":6112026}}`)}},
		{"retornoData ISO date", []string{item(`"9001"`, `"status":{"retornoProgramado":"S","retornoData":"2026-11-06"}`)}},
		{"retornoData missing", []string{item(`"9001"`, `"status":{"retornoProgramado":"S"}`)}},
		{"retornoProgramado lower-case s", []string{item(`"9001"`, `"status":{"retornoProgramado":"s","retornoData":"06/11/2026"}`)}},
		{"retornoProgramado boolean", []string{item(`"9001"`, `"status":{"retornoProgramado":true}`)}},
		{"retornoAtrasado values", []string{
			item(`"9001"`, `"status":{"retornoAtrasado":"S"}`),
			item(`"9002"`, `"status":{"retornoAtrasado":"s"}`),
			item(`"9003"`, `"status":{"retornoAtrasado":"N"}`),
			item(`"9004"`, `"status":{}`),
			item(`"9005"`, `"status":{"retornoAtrasado":" S "}`),
		}},
		{"overdue without a date", []string{item(`"9001"`, `"status":{"retornoProgramado":"N","retornoAtrasado":"S"}`)}},
		{"usuarioAtribuido variants", []string{
			item(`"9001"`, `"usuarioAtribuido":null`),
			item(`"9002"`, `"usuarioAtribuido":{}`),
			item(`"9003"`, `"usuarioAtribuido":{"idUsuario":"0","sigla":""}`),
			item(`"9004"`, `"usuarioAtribuido":{"sigla":" fulano.tal "}`),
			item(`"9005"`, `"usuarioAtribuido":{"idUsuario":"7","sigla":12}`),
			item(`"9006"`, `"usuarioAtribuido":""`),
			item(`"9007"`, `"usuarioAtribuido":false`),
			item(`"9008"`, `"usuarioAtribuido":[]`),
			item(`"9009"`, `"usuarioAtribuido":{"idUsuario":"abc","sigla":"x"}`),
			item(`"9010"`, `"usuarioAtribuido":{"idUsuario":"7","sigla":"","nome":" Fulano &amp; Tal "}`),
			item(`"9011"`, `"usuarioAtribuido":{"idUsuario":0,"sigla":"  "}`),
			item(`"9012"`, `"usuarioAtribuido":{"IDUSUARIO":"8","SIGLA":"beltrano","NOME":"Beltrano"}`),
			item(`"9013"`, `"usuarioAtribuido":"fulano.tal"`),
			item(`"9014"`, `"usuarioAtribuido":{"idUsuario":"9","sigla":null,"nome":null}`),
		}},
		{"entities and spaces in every process field", []string{item(`"9001"`,
			`"numero":" 00000.000001/2026-00 &amp; ","tipoProcesso":" Administra&ccedil;&atilde;o &amp; Di&aacute;rias ","descricao":" Pedido &amp; &quot;viagem&quot; ",`+
				`"usuarioAtribuido":{"idUsuario":"101","sigla":" fulano&amp;tal ","nome":" Fulano &amp; Tal "},`+
				`"status":{"retornoProgramado":"S","retornoData":{"dataProgramada":"06/11/2026"},"retornoAtrasado":"N","processoGeradoRecebido":" R&amp; "}`)}},
		{"invalid bytes in strings", []string{item(`"9001"`, `"numero":"a`+"\xff"+`b","descricao":"x`+"\xe2\x82"+`y","tipoProcesso":"`+"\xe2\x82\xff"+`"`)}},
		{"number where text is expected", []string{item(`"9001"`, `"numero":123`)}},
		{"null text fields", []string{item(`"9001"`, `"numero":null,"tipoProcesso":null,"descricao":null`)}},
		{"repeated attribute key, last wins", []string{item(`"9001"`, `"numero":"first","numero":"second"`)}},
		{"multi-byte text before entities", []string{item(`"9001"`, `"descricao":"ção ção &amp;&#8211;&ndash ção"`)}},
	}
	inputs := make([]wireInput, len(cases))
	for i, c := range cases {
		inputs[i] = wireInput{name: c.name, route: "processes", zone: "America/Boa_Vista", status: http.StatusOK, body: pageBody(c.items...)}
	}
	return inputs
}

func activityInputs() []wireInput {
	cases := []struct {
		name  string
		zone  string
		items []string
	}{
		{"activity without hora", "America/Boa_Vista", []string{item(`"50001"`, `"idProcesso":"9001","usuario":"fulano.tal","data":"04/11/2026","unidade":"SEDEMO/NUPROC","informacao":"x"`)}},
		{"activity in the 2018 gap in Sao Paulo", "America/Sao_Paulo", []string{item(`"50001"`, `"data":"04/11/2018","hora":"00:30:00"`)}},
		{"activity in the 2019 overlap in Sao Paulo", "America/Sao_Paulo", []string{item(`"50001"`, `"data":"16/02/2019","hora":"23:30:00"`)}},
		{"activity in the 2018 gap in Boa Vista", "America/Boa_Vista", []string{item(`"50001"`, `"data":"04/11/2018","hora":"00:30:00"`)}},
		{"activity at the last second of the year in Boa Vista", "America/Boa_Vista", []string{item(`"50001"`, `"data":"31/12/2026","hora":"23:59:59"`)}},
		{"activity in UTC", "UTC", []string{item(`"50001"`, `"data":"31/12/2026","hora":"23:59:59"`)}},
		{"activity with usuario null", "America/Boa_Vista", []string{item(`"50001"`, `"usuario":null,"data":"04/11/2026","hora":"13:13:05"`)}},
		{"activity procedure ids", "America/Boa_Vista", []string{
			item(`"50001"`, `"data":"04/11/2026","hora":"13:13:05"`),
			item(`"50002"`, `"idProcesso":"0","data":"04/11/2026","hora":"13:13:05"`),
			item(`"50003"`, `"idProcesso":"123","data":"04/11/2026","hora":"13:13:05"`),
		}},
		{"activity date and time with spaces", "America/Boa_Vista", []string{item(`"50001"`, `"data":" 04/11/2026 ","hora":" 13:13:05 "`)}},
		{"activity date and time that do not parse", "America/Boa_Vista", []string{
			item(`"50001"`, `"data":"4/11/2026","hora":"13:13:05"`),
			item(`"50002"`, `"data":"04/11/2026","hora":"13:13"`),
			item(`"50003"`, `"data":"04/11/2026","hora":"24:00:00"`),
			item(`"50004"`, `"data":"29/02/2025","hora":"10:00:00"`),
			item(`"50005"`, `"data":"2026-11-04","hora":"10:00:00"`),
			item(`"50006"`, `"data":"04/11/2026","hora":"9:00:00"`),
			item(`"50007"`, `"data":"04/11/2026","hora":"09:00:00.5"`),
		}},
		{"entities and spaces in every activity field", "America/Boa_Vista", []string{item(`"50001"`,
			`"usuario":" fulano&amp;tal ","unidade":" SEDEMO&amp;NUPROC ","informacao":" Documento &quot;Of&iacute;cio&quot; &amp; anexo ","data":"04/11/2026","hora":"13:13:05"`)}},
		{"activity with invalid bytes", "America/Boa_Vista", []string{item(`"50001"`, `"informacao":"a`+"\xff"+`b`+"\xe2\x82"+`","usuario":"`+"\xe2\x82"+`"`)}},
		{"activity number where text is expected", "America/Boa_Vista", []string{item(`"50001"`, `"usuario":7`)}},
	}
	inputs := make([]wireInput, len(cases))
	for i, c := range cases {
		inputs[i] = wireInput{name: c.name, route: "activities", zone: c.zone, status: http.StatusOK, body: pageBody(c.items...)}
	}
	return inputs
}

func documentInputs() []wireInput {
	cases := []struct {
		name  string
		items []string
	}{
		{"document names", []string{
			item(`"80001"`, `"protocoloFormatado":"0012345","tipo":"Of&iacute;cio","titulo":"12","nome":"oficio.pdf","nomeComposto":"Of&iacute;cio 12 (0012345)"`),
			item(`"80002"`, `"protocoloFormatado":"0012346","tipo":" Of&iacute;cio ","titulo":" 12 ","nome":"oficio.pdf","nomeComposto":"  "`),
			item(`"80003"`, `"protocoloFormatado":"0012347","tipo":"","titulo":"","nome":" planilha.pdf "`),
			item(`"80004"`, `"protocoloFormatado":"0012348","tipo":"","titulo":"12"`),
			item(`"80005"`, `"protocoloFormatado":"0012349","tipo":"Anexo","titulo":""`),
			item(`"80006"`, `"protocoloFormatado":"0012350"`),
			item(`"80007"`, `"protocoloFormatado":"0012351","nome":null,"titulo":null,"tipo":null,"nomeComposto":null`),
		}},
		{"entities and spaces in every document field", []string{
			item(`"80001"`, `"protocoloFormatado":" 00123&amp;45 ","tipo":" Of&iacute;cio &amp; ","titulo":" 12&amp; ","nome":" a&amp;b.pdf ","nomeComposto":" Of&iacute;cio &amp; 12 "`),
			item(`"80002"`, `"protocoloFormatado":"1","tipo":" Of&iacute;cio &amp; ","titulo":" 12&amp; ","nome":" a&amp;b.pdf "`),
			item(`"80003"`, `"protocoloFormatado":"2","nome":" a&amp;b.pdf "`),
		}},
		{"document procedure ids", []string{
			item(`"80001"`, `"idProcedimento":"9002"`),
			item(`"80002"`, `"idProcedimento":"0"`),
			item(`"80003"`, ``),
		}},
		{"document with invalid bytes", []string{item(`"80001"`, `"tipo":"a`+"\xff"+`","titulo":"`+"\xe2\x82"+`b"`)}},
		{"document number where text is expected", []string{item(`"80001"`, `"titulo":12`)}},
	}
	inputs := make([]wireInput, len(cases))
	for i, c := range cases {
		inputs[i] = wireInput{name: c.name, route: "documents", zone: "America/Boa_Vista", status: http.StatusOK, body: pageBody(c.items...)}
	}
	return inputs
}

type goldenOrg struct {
	ID      int64  `json:"id"`
	Acronym string `json:"acronym"`
}

type orgsPageCase struct {
	Name        string      `json:"name"`
	ContentType string      `json:"content_type"`
	Body        []byte      `json:"body_base64"`
	Orgs        []goldenOrg `json:"orgs"`
	Error       *string     `json:"error"`
}

func orgsPageVectors(t *testing.T) []orgsPageCase {
	cases := orgsPageInputs(t)
	for i := range cases {
		orgs, err := listOrgsServing(t, cases[i].ContentType, cases[i].Body)
		if err != nil {
			text := err.Error()
			cases[i].Error = &text
			continue
		}
		cases[i].Orgs = make([]goldenOrg, len(orgs))
		for j, org := range orgs {
			cases[i].Orgs[j] = goldenOrg(org)
		}
	}
	return cases
}

func listOrgsServing(t *testing.T, contentType string, body []byte) ([]Org, error) {
	t.Helper()
	mux := http.NewServeMux()
	mux.HandleFunc("GET /sei/{$}", func(w http.ResponseWriter, _ *http.Request) {
		w.Header()["Content-Type"] = nil
		if contentType != "" {
			w.Header().Set("Content-Type", contentType)
		}
		_, _ = w.Write(body)
	})
	server := httptest.NewServer(mux)
	defer server.Close()
	return ListOrgs(t.Context(), server.URL, Options{UserAgent: "sei-monitor/golden"})
}

func selectPage(options string) []byte {
	return []byte(`<html><body><select id="selOrgao">` + options + `</select></body></html>`)
}

func orgsPageInputs(t *testing.T) []orgsPageCase {
	login := fixture(t, "login.html")
	utf8Page := []byte(`<html><head><meta charset="utf-8"></head><body><select id="selOrgao"><option value="77">ÓRGÃO</option><option value="78">CONCEIÇÃO</option></select></body></html>`)
	sniffedUTF8 := []byte(`<html><body><select id="selOrgao"><option value="77">ÓRGÃO</option></select></body></html>`)
	latin1 := []byte("<html><body><select id=\"selOrgao\"><option value=\"77\">\xd3RG\xc3O \x80</option></select></body></html>")

	splitHead := `<html><head><title>x</title></head><body><p>`
	splitRune := []byte(splitHead + strings.Repeat("a", 1023-len(splitHead)) + `Ó</p><select id="selOrgao"><option value="77">ÓRGÃO</option></select></body></html>`)
	splitAfterUTF8 := []byte(splitHead + "ç" + strings.Repeat("a", 1021-len(splitHead)) + `Ó</p><select id="selOrgao"><option value="77">ÓRGÃO</option></select></body></html>`)
	lateMeta := []byte(`<html><head><!--` + strings.Repeat("a", 1100) + `--><meta charset="iso-8859-1"></head><body><select id="selOrgao"><option value="77">ÓRGÃO</option></select></body></html>`)
	lateMetaLatin1 := []byte(`<html><head><!--` + strings.Repeat("a", 1100) + `--><meta charset="utf-8"></head><body><select id="selOrgao"><option value="77">` + "\xd3RG\xc3O" + `</option></select></body></html>`)

	return []orgsPageCase{
		{Name: "login.html with an ISO-8859-1 header", ContentType: "text/html; charset=iso-8859-1", Body: login},
		{Name: "login.html with only the meta tag", Body: login},
		{Name: "login.html with a text/html header and no charset", ContentType: "text/html", Body: login},
		{Name: "UTF-8 with charset=utf-8", ContentType: "text/html; charset=utf-8", Body: utf8Page},
		{Name: "UTF-8 with a quoted upper-case charset", ContentType: `text/html; charset="UTF-8"`, Body: utf8Page},
		{Name: "UTF-8 without any indication", Body: sniffedUTF8},
		{Name: "UTF-8 without any indication, a rune split at byte 1024", Body: splitRune},
		{Name: "UTF-8 without any indication, a rune split at byte 1024 after another one", Body: splitAfterUTF8},
		{Name: "meta tag after the first 1024 bytes over UTF-8", Body: lateMeta},
		{Name: "meta tag after the first 1024 bytes over Latin-1", Body: lateMetaLatin1},
		{Name: "Latin-1 without any indication, with byte 0x80", Body: latin1},
		{Name: "ISO-8859-1 header over byte 0x80", ContentType: "text/html; charset=iso-8859-1", Body: latin1},
		{Name: "UTF-8 BOM against an ISO-8859-1 header", ContentType: "text/html; charset=iso-8859-1", Body: append([]byte("\xef\xbb\xbf"), sniffedUTF8...)},
		{Name: "unknown charset in the header, meta tag decides", ContentType: "text/html; charset=x-unknown-sei", Body: login},
		{Name: "unknown charset in the header over UTF-8", ContentType: "text/html; charset=x-unknown-sei", Body: sniffedUTF8},
		{Name: "meta charset windows-1252", Body: []byte("<html><head><meta charset=\"windows-1252\"></head><body><select id=\"selOrgao\"><option value=\"77\">\xd3RG\xc3O \x80</option></select></body></html>")},
		{Name: "select by name without id", Body: []byte(`<select name="selOrgao"><option value="1">ADEMO</option></select>`)},
		{Name: "two selects, the first wins", Body: []byte(`<select id="other"><option value="9">OUTRO</option></select><select id="selOrgao"><option value="1">ADEMO</option></select><select name="selOrgao"><option value="2">BDEMO</option></select>`)},
		{Name: "id in another case does not match", Body: []byte(`<SELECT ID="SELORGAO"><option value="1">ADEMO</option></SELECT><select ID="selOrgao"><OPTION VALUE="2">BDEMO</OPTION></select>`)},
		{Name: "option values", Body: selectPage(`<option value=" 63 ">A</option><option value="null">B</option><option value="0">C</option><option value="-5">D</option><option value="+7">E</option><option value="7x">F</option><option>G</option><option value="">H</option><option value="9223372036854775808">I</option><option value="0063">J</option><option value="1e3">K</option>`)},
		{Name: "option text with nested tags and spaces", Body: selectPage("<option value=\"5\">  A <b>B</b>\n\t C  <i>D <u>E</u></i> </option><option value=\"6\"></option><option value=\"7\">&nbsp;</option>")},
		{Name: "option text with entities", Body: selectPage(`<option value="5">&Oacute;RG&Atilde;O &amp; CIA&nbsp;X</option>`)},
		{Name: "options inside optgroup", Body: selectPage(`<optgroup label="g"><option value="1">ADEMO</option></optgroup><option value="2">BDEMO</option>`)},
		{Name: "no select", Body: []byte(`<html><body><form><input name="selOrgao" value="1"></form></body></html>`)},
		{Name: "select without valid options", Body: selectPage(`<option value="null">&nbsp;</option>`)},
		{Name: "empty body", Body: []byte{}},
	}
}
