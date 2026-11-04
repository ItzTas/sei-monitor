package seitest

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
	_ "time/tzdata"

	"golang.org/x/text/encoding/charmap"
)

const (
	Username      = "fulano.tal"
	Password      = "senha-de-teste"
	UserID        = 101
	UserName      = "Fulano de Tal"
	OrgID         = 63
	OrgAcronym    = "SEDEMO"
	UnitNUPROC    = 1001
	UnitGAB       = 1002
	SEIVersion    = "5.0.5"
	ModuleVersion = "3.0.4"
	TimeZone      = "America/Boa_Vista"

	APIPath = "/sei/modulos/wssei/controlador_ws.php/api/v2"

	RouteLogin      = "/autenticar"
	RouteVersion    = "/versao"
	RouteOrgs       = "/orgao/listar"
	RouteProcesses  = "/processo/listar"
	RouteActivities = "/atividade/listar"
	RouteDocuments  = "/documento/listar"
	RouteLoginPage  = "/sip/login.php"
)

type Org struct {
	ID          int64
	Acronym     string
	Description string
}

type Unit struct {
	ID          int64
	Acronym     string
	Description string
}

type User struct {
	ID      int64
	Acronym string
	Name    string
}

type Account struct {
	Username string
	Password string
	OrgID    int64
	User     User
	Units    []Unit
}

type Process struct {
	ID             int64
	Number         string
	Type           string
	Description    string
	Assignee       *User
	ReturnDate     string
	ReturnOverdue  bool
	Generated      bool
	GeneratingUnit Unit
}

type Activity struct {
	ID   int64
	User string
	Unit string
	Text string
	At   time.Time
}

type Document struct {
	ID       int64
	Protocol string
	Type     string
	Number   string
	Label    string
}

type Server struct {
	http     *httptest.Server
	location *time.Location

	mu           sync.Mutex
	orgs         []Org
	accounts     []Account
	sessions     map[string]int
	processes    map[int64][]Process
	activities   map[int64][]Activity
	documents    map[int64][]Document
	calls        map[string]int
	failures     map[string][]int
	inactive     bool
	logins       int
	failedLogins int
	userAgent    string
}

func New(tb testing.TB) *Server {
	tb.Helper()
	location, err := time.LoadLocation(TimeZone)
	if err != nil {
		tb.Fatalf("seitest: load time zone %s: %v", TimeZone, err)
	}
	s := &Server{
		location:   location,
		orgs:       DefaultOrgs(),
		accounts:   []Account{DefaultAccount()},
		sessions:   map[string]int{},
		processes:  map[int64][]Process{},
		activities: map[int64][]Activity{},
		documents:  map[int64][]Document{},
		calls:      map[string]int{},
		failures:   map[string][]int{},
	}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /sei/{$}", s.redirectToLogin)
	mux.HandleFunc("GET "+RouteLoginPage, s.loginPage)
	mux.HandleFunc("GET "+APIPath+RouteVersion, s.api(RouteVersion, true, s.version))
	mux.HandleFunc("POST "+APIPath+RouteLogin, s.api(RouteLogin, false, s.login))
	mux.HandleFunc("GET "+APIPath+RouteOrgs, s.api(RouteOrgs, false, s.orgList))
	mux.HandleFunc("GET "+APIPath+RouteProcesses, s.api(RouteProcesses, true, s.processList))
	mux.HandleFunc("GET "+APIPath+RouteActivities, s.api(RouteActivities, true, s.activityList))
	mux.HandleFunc("GET "+APIPath+RouteDocuments+"/{id}", s.api(RouteDocuments, true, s.documentList))
	s.http = httptest.NewServer(mux)
	tb.Cleanup(s.http.Close)
	return s
}

func DefaultOrgs() []Org {
	return []Org{
		{ID: 63, Acronym: OrgAcronym, Description: "Secretaria de Demonstração"},
		{ID: 34, Acronym: "ADEMO", Description: "Agência de Demonstração"},
		{ID: 48, Acronym: "BDEMO", Description: "Banco de Demonstração"},
		{ID: 12, Acronym: "CDEMO", Description: "Corpo de Demonstração"},
		{ID: 15, Acronym: "DDEMO", Description: "Departamento de Demonstração"},
		{ID: 21, Acronym: "EDEMO", Description: "Escritório de Demonstração"},
		{ID: 25, Acronym: "FDEMO", Description: "Fundação de Demonstração"},
		{ID: 40, Acronym: "GDEMO", Description: "Gabinete de Demonstração"},
		{ID: 50, Acronym: "HDEMO", Description: "Hospital de Demonstração"},
		{ID: 70, Acronym: "IDEMO", Description: "Instituto de Demonstração"},
		{ID: 72, Acronym: "JDEMO", Description: "Junta de Demonstração"},
		{ID: 80, Acronym: "KDEMO", Description: "Kit de Demonstração"},
	}
}

func DefaultAccount() Account {
	return Account{
		Username: Username,
		Password: Password,
		OrgID:    OrgID,
		User:     User{ID: UserID, Acronym: Username, Name: UserName},
		Units: []Unit{
			{ID: UnitNUPROC, Acronym: "SEDEMO/NUPROC", Description: "Núcleo de Processos"},
			{ID: UnitGAB, Acronym: "SEDEMO/GAB", Description: "Gabinete"},
		},
	}
}

func ProcessNumber(n int) string {
	return fmt.Sprintf("00000.%06d/2026-00", n)
}

func (s *Server) URL() string {
	return s.http.URL
}

func (s *Server) APIURL() string {
	return s.http.URL + APIPath
}

func (s *Server) Location() *time.Location {
	return s.location
}

func (s *Server) SetProcesses(unitID int64, processes ...Process) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.processes[unitID] = slices.Clone(processes)
}

func (s *Server) AddProcess(unitID int64, process Process) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.processes[unitID] = append(s.processes[unitID], process)
}

func (s *Server) RemoveProcess(unitID, procedureID int64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.processes[unitID] = slices.DeleteFunc(s.processes[unitID], func(p Process) bool { return p.ID == procedureID })
}

func (s *Server) UpdateProcess(unitID, procedureID int64, change func(*Process)) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i := range s.processes[unitID] {
		if s.processes[unitID][i].ID == procedureID {
			change(&s.processes[unitID][i])
		}
	}
}

func (s *Server) AddActivities(procedureID int64, activities ...Activity) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.activities[procedureID] = append(s.activities[procedureID], activities...)
}

func (s *Server) AddDocuments(procedureID int64, documents ...Document) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.documents[procedureID] = append(s.documents[procedureID], documents...)
}

func (s *Server) RemoveDocument(procedureID, documentID int64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.documents[procedureID] = slices.DeleteFunc(s.documents[procedureID], func(d Document) bool { return d.ID == documentID })
}

func (s *Server) ExpireTokens() {
	s.mu.Lock()
	defer s.mu.Unlock()
	clear(s.sessions)
}

func (s *Server) SetPassword(password string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.accounts[0].Password = password
	clear(s.sessions)
}

func (s *Server) FailNext(route string, status, times int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for range times {
		s.failures[route] = append(s.failures[route], status)
	}
}

func (s *Server) SetModuleInactive(inactive bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.inactive = inactive
}

func (s *Server) Calls(route string) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.calls[route]
}

func (s *Server) Logins() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.logins
}

func (s *Server) FailedLogins() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.failedLogins
}

func (s *Server) LastUserAgent() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.userAgent
}

type handler func(w http.ResponseWriter, r *http.Request, account *Account, unit Unit)

func (s *Server) api(route string, authenticated bool, next handler) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		s.mu.Lock()
		defer s.mu.Unlock()
		s.calls[route]++
		s.userAgent = r.UserAgent()
		if s.inactive {
			writeJSON(w, http.StatusUnauthorized, map[string]any{"sucesso": false, "mensagem": "Módulo inativo.", "exception": nil})
			return
		}
		if status, ok := s.popFailure(route); ok {
			writeFailure(w, status)
			return
		}
		if !authenticated {
			next(w, r, nil, Unit{})
			return
		}
		account, unit, ok := s.authenticate(w, r)
		if ok {
			next(w, r, account, unit)
		}
	}
}

func (s *Server) authenticate(w http.ResponseWriter, r *http.Request) (*Account, Unit, bool) {
	token := r.Header.Get("token")
	if token == "" {
		writeJSON(w, http.StatusUnauthorized, failure("Acesso negado!"))
		return nil, Unit{}, false
	}
	index, ok := s.sessions[token]
	if !ok {
		writeJSON(w, http.StatusForbidden, failure("Token inválido!"))
		return nil, Unit{}, false
	}
	account := &s.accounts[index]
	header := r.Header.Get("unidade")
	if header == "" {
		return account, account.Units[0], true
	}
	for _, unit := range account.Units {
		if strconv.FormatInt(unit.ID, 10) == header {
			return account, unit, true
		}
	}
	writeJSON(w, http.StatusOK, failure("Unidade não permitida para o usuário."))
	return nil, Unit{}, false
}

func (s *Server) popFailure(route string) (int, bool) {
	queue := s.failures[route]
	if len(queue) == 0 {
		return 0, false
	}
	s.failures[route] = queue[1:]
	return queue[0], true
}

func writeFailure(w http.ResponseWriter, status int) {
	if status >= http.StatusInternalServerError {
		w.Header().Set("Content-Type", "text/html; charset=iso-8859-1")
		w.WriteHeader(status)
		_, _ = w.Write([]byte("<html><body><h1>Erro interno</h1></body></html>"))
		return
	}
	writeJSON(w, status, failure("Falha simulada pelo seitest."))
}

func (s *Server) login(w http.ResponseWriter, r *http.Request, _ *Account, _ Unit) {
	if err := r.ParseForm(); err != nil {
		writeJSON(w, http.StatusOK, failure("Requisição inválida."))
		return
	}
	for index, account := range s.accounts {
		if account.Username != r.PostForm.Get("usuario") || account.Password != r.PostForm.Get("senha") || strconv.FormatInt(account.OrgID, 10) != r.PostForm.Get("orgao") {
			continue
		}
		token := randomToken()
		s.sessions[token] = index
		s.logins++
		writeJSON(w, http.StatusOK, success(loginData(account, token), nil))
		return
	}
	s.failedLogins++
	writeJSON(w, http.StatusOK, failure("Usuário ou Senha Inválida."))
}

func loginData(account Account, token string) map[string]any {
	units := make([]map[string]any, 0, len(account.Units))
	for _, unit := range account.Units {
		units = append(units, map[string]any{"id": id(unit.ID), "sigla": unit.Acronym, "descricao": unit.Description})
	}
	return map[string]any{
		"loginData": map[string]any{
			"IdSistema":      "100000100",
			"IdContexto":     nil,
			"IdUsuario":      id(account.User.ID),
			"IdUnidadeAtual": id(account.Units[0].ID),
			"sigla":          account.User.Acronym,
			"nome":           account.User.Name,
		},
		"perfis":        []map[string]any{{"IdPerfil": "100000938", "Nome": "Básico"}},
		"unidades":      units,
		"identificador": account.User.Acronym + "@" + OrgAcronym,
		"token":         token,
	}
}

func (s *Server) version(w http.ResponseWriter, _ *http.Request, _ *Account, _ Unit) {
	writeJSON(w, http.StatusOK, success(map[string]any{"sei": SEIVersion, "wssei": ModuleVersion}, nil))
}

func (s *Server) orgList(w http.ResponseWriter, _ *http.Request, _ *Account, _ Unit) {
	sorted := slices.SortedFunc(slices.Values(s.orgs), func(a, b Org) int { return strings.Compare(a.Acronym, b.Acronym) })
	data := make([]map[string]any, 0, 10)
	for _, org := range sorted[:min(10, len(sorted))] {
		data = append(data, map[string]any{"id": id(org.ID), "sigla": org.Acronym, "descricao": org.Description})
	}
	total := len(s.orgs)
	writeJSON(w, http.StatusOK, success(data, &total))
}

func (s *Server) processList(w http.ResponseWriter, r *http.Request, _ *Account, unit Unit) {
	page, limit := pagination(r)
	kind := r.URL.Query().Get("tipo")
	processes := slices.DeleteFunc(slices.Clone(s.processes[unit.ID]), func(p Process) bool {
		return (kind == "G" && !p.Generated) || (kind == "R" && p.Generated)
	})
	total := len(processes)
	var carried any
	data := make([]map[string]any, 0, limit)
	for _, process := range paginate(processes, page, limit) {
		data = append(data, processItem(process, unit, &carried))
	}
	writeJSON(w, http.StatusOK, success(data, &total))
}

func processItem(process Process, unit Unit, carried *any) map[string]any {
	returnFlag := "N"
	if process.ReturnDate != "" {
		returnFlag = "S"
		*carried = map[string]any{"dataProgramada": process.ReturnDate, "unidade": "SEDEMO/GAB"}
	}
	var assignee any
	if process.Assignee != nil {
		assignee = map[string]any{
			"idAtividade":   "700",
			"idUsuario":     id(process.Assignee.ID),
			"sigla":         process.Assignee.Acronym,
			"nome":          process.Assignee.Name,
			"nomeformatado": process.Assignee.Acronym + " - " + process.Assignee.Name,
		}
	}
	generatingUnit := process.GeneratingUnit
	if generatingUnit.ID == 0 {
		generatingUnit = Unit{ID: 1500, Acronym: "SEDEMO/PROTOCOLO"}
	}
	origin := "R"
	if process.Generated {
		origin = "G"
		generatingUnit = unit
	}
	return map[string]any{
		"id":                 id(process.ID),
		"status":             "P",
		"seiNumMaxDocsPasta": "20",
		"atributos": map[string]any{
			"idProcedimento":   id(process.ID),
			"idProtocolo":      id(process.ID),
			"numero":           process.Number,
			"tipoProcesso":     process.Type,
			"descricao":        process.Description,
			"usuarioAtribuido": assignee,
			"unidade":          map[string]any{"idUnidade": id(generatingUnit.ID), "sigla": generatingUnit.Acronym},
			"ciencias":         []any{},
			"marcador":         []any{},
			"dadosAbertura":    []any{},
			"anotacoes":        []any{},
			"status": map[string]any{
				"documentoSigiloso":      "",
				"documentoRestrito":      "N",
				"documentoNovo":          "N",
				"retornoProgramado":      returnFlag,
				"retornoData":            *carried,
				"retornoAtrasado":        yesNo(process.ReturnOverdue),
				"processoAberto":         "S",
				"processoSobrestado":     "N",
				"tipo":                   nil,
				"processoGeradoRecebido": origin,
			},
		},
	}
}

func (s *Server) activityList(w http.ResponseWriter, r *http.Request, _ *Account, _ Unit) {
	procedureID, err := strconv.ParseInt(r.URL.Query().Get("procedimento"), 10, 64)
	if err != nil {
		writeJSON(w, http.StatusOK, failure("O procedimento deve ser informado!"))
		return
	}
	page, limit := pagination(r)
	newestFirst := slices.Clone(s.activities[procedureID])
	slices.Reverse(newestFirst)
	data := make([]map[string]any, 0, limit)
	for _, activity := range paginate(newestFirst, page, limit) {
		local := activity.At.In(s.location)
		var user any
		if activity.User != "" {
			user = activity.User
		}
		data = append(data, map[string]any{
			"id": id(activity.ID),
			"atributos": map[string]any{
				"idProcesso": id(procedureID),
				"usuario":    user,
				"data":       local.Format("02/01/2006"),
				"hora":       local.Format("15:04:05"),
				"unidade":    activity.Unit,
				"informacao": activity.Text,
			},
		})
	}
	total := len(newestFirst)
	writeJSON(w, http.StatusOK, success(data, &total))
}

func (s *Server) documentList(w http.ResponseWriter, r *http.Request, _ *Account, _ Unit) {
	procedureID, err := strconv.ParseInt(r.PathValue("id"), 10, 64)
	if err != nil {
		writeJSON(w, http.StatusOK, failure("O procedimento deve ser informado!"))
		return
	}
	page, limit := pagination(r)
	documents := s.documents[procedureID]
	data := make([]map[string]any, 0, limit)
	for _, document := range paginate(documents, page, limit) {
		data = append(data, map[string]any{
			"id": id(document.ID),
			"atributos": map[string]any{
				"idProcedimento":     id(procedureID),
				"idProtocolo":        id(document.ID),
				"protocoloFormatado": document.Protocol,
				"nome":               nil,
				"titulo":             document.Number,
				"tipo":               document.Type,
				"tipoDocumento":      "I",
				"mimeType":           "html",
				"nomeComposto":       document.Label,
				"status":             map[string]any{"documentoAssinado": "S"},
			},
		})
	}
	total := len(documents)
	writeJSON(w, http.StatusOK, success(data, &total))
}

func (s *Server) redirectToLogin(w http.ResponseWriter, r *http.Request) {
	http.Redirect(w, r, RouteLoginPage+"?sigla_orgao_sistema="+OrgAcronym+"&sigla_sistema=SEI", http.StatusFound)
}

func (s *Server) loginPage(w http.ResponseWriter, r *http.Request) {
	s.mu.Lock()
	s.calls[RouteLoginPage]++
	s.userAgent = r.UserAgent()
	orgs := slices.Clone(s.orgs)
	s.mu.Unlock()

	var options strings.Builder
	options.WriteString(`<option value="null">&nbsp;</option>`)
	for _, org := range orgs {
		fmt.Fprintf(&options, `<option value="%d">%s</option>`, org.ID, org.Acronym)
	}
	page := `<!DOCTYPE html><html><head><meta http-equiv="Content-Type" content="text/html; charset=iso-8859-1">` +
		`<title>SIP - Autenticação de Usuário</title></head><body><form id="frmLogin">` +
		`<select id="selOrgao" name="selOrgao" class="infraSelect">` + options.String() + `</select></form></body></html>`
	encoded, err := charmap.ISO8859_1.NewEncoder().String(page)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=iso-8859-1")
	_, _ = w.Write([]byte(encoded))
}

func pagination(r *http.Request) (int, int) {
	query := r.URL.Query()
	limit, err := strconv.Atoi(query.Get("limit"))
	if err != nil || limit <= 0 {
		limit = 10
	}
	page, err := strconv.Atoi(query.Get("start"))
	if err != nil || page < 0 {
		page = 0
	}
	return page, limit
}

func paginate[T any](items []T, page, limit int) []T {
	start := page * limit
	if start >= len(items) {
		return nil
	}
	return items[start:min(start+limit, len(items))]
}

func success(data any, total *int) map[string]any {
	body := map[string]any{"sucesso": true}
	if !empty(data) {
		body["data"] = data
	}
	if total != nil {
		body["total"] = strconv.Itoa(*total)
	}
	return body
}

func empty(data any) bool {
	switch value := data.(type) {
	case []map[string]any:
		return len(value) == 0
	case nil:
		return true
	}
	return false
}

func failure(message string) map[string]any {
	return map[string]any{"sucesso": false, "mensagem": message, "exception": "Exceção salva no SEI"}
}

func writeJSON(w http.ResponseWriter, status int, body any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(body)
}

func id(n int64) string {
	return strconv.FormatInt(n, 10)
}

func yesNo(value bool) string {
	if value {
		return "S"
	}
	return "N"
}

func randomToken() string {
	buf := make([]byte, 24)
	_, _ = rand.Read(buf)
	return hex.EncodeToString(buf)
}
