package sei

import (
	"bytes"
	"encoding/json"
	"fmt"
	"html"
	"math"
	"strconv"
	"strings"
	"time"
)

const (
	dateLayout     = "02/01/2006"
	dateTimeLayout = "02/01/2006 15:04:05"
)

type envelope struct {
	Success *bool           `json:"sucesso"`
	Message json.RawMessage `json:"mensagem"`
	Data    json.RawMessage `json:"data"`
	Total   json.RawMessage `json:"total"`
}

func (e envelope) message() string {
	var text string
	if json.Unmarshal(e.Message, &text) == nil {
		return html.UnescapeString(strings.TrimSpace(text))
	}
	var list []string
	if json.Unmarshal(e.Message, &list) != nil {
		return ""
	}
	for i, item := range list {
		list[i] = html.UnescapeString(strings.TrimSpace(item))
	}
	return strings.Join(list, "; ")
}

func (e envelope) total() int {
	var total flexInt
	if json.Unmarshal(e.Total, &total) != nil {
		return 0
	}
	return int(total)
}

func (e envelope) list(target any) error {
	if isEmptyJSON(e.Data) {
		return nil
	}
	return json.Unmarshal(e.Data, target)
}

func isEmptyJSON(raw json.RawMessage) bool {
	trimmed := bytes.TrimSpace(raw)
	return len(trimmed) == 0 || bytes.Equal(trimmed, []byte("null")) || bytes.Equal(trimmed, []byte("false")) || bytes.Equal(trimmed, []byte(`""`))
}

type flexInt int64

func (f *flexInt) UnmarshalJSON(raw []byte) error {
	text := strings.TrimSpace(string(raw))
	if unquoted, err := strconv.Unquote(text); err == nil {
		text = strings.TrimSpace(unquoted)
	}
	if text == "" || text == "null" {
		*f = 0
		return nil
	}
	if n, err := strconv.ParseInt(text, 10, 64); err == nil {
		*f = flexInt(n)
		return nil
	}
	n, err := strconv.ParseFloat(text, 64)
	if err != nil || n != math.Trunc(n) || math.Abs(n) > math.MaxInt64 {
		return fmt.Errorf("sei: expected a whole number, got %s", raw)
	}
	*f = flexInt(n)
	return nil
}

type loginWire struct {
	Token     string `json:"token"`
	LoginData struct {
		UserID        flexInt `json:"IdUsuario"`
		CurrentUnitID flexInt `json:"IdUnidadeAtual"`
		Acronym       string  `json:"sigla"`
		Name          string  `json:"nome"`
	} `json:"loginData"`
	Units []struct {
		ID          flexInt `json:"id"`
		Acronym     string  `json:"sigla"`
		Description string  `json:"descricao"`
	} `json:"unidades"`
}

func (w loginWire) session() Session {
	session := Session{
		UserID:        int64(w.LoginData.UserID),
		UserAcronym:   w.LoginData.Acronym,
		UserName:      html.UnescapeString(w.LoginData.Name),
		CurrentUnitID: int64(w.LoginData.CurrentUnitID),
	}
	for _, unit := range w.Units {
		session.Units = append(session.Units, Unit{
			ID:          int64(unit.ID),
			Acronym:     unit.Acronym,
			Description: html.UnescapeString(unit.Description),
		})
	}
	return session
}

type versionWire struct {
	SEI    string `json:"sei"`
	Module string `json:"wssei"`
}

type processWire struct {
	ID         flexInt `json:"id"`
	Attributes struct {
		ProcedureID flexInt         `json:"idProcedimento"`
		Number      string          `json:"numero"`
		Type        string          `json:"tipoProcesso"`
		Description string          `json:"descricao"`
		Assignee    json.RawMessage `json:"usuarioAtribuido"`
		Status      struct {
			Return        string          `json:"retornoProgramado"`
			ReturnData    json.RawMessage `json:"retornoData"`
			ReturnOverdue string          `json:"retornoAtrasado"`
			Origin        string          `json:"processoGeradoRecebido"`
		} `json:"status"`
	} `json:"atributos"`
}

func (w processWire) process() Process {
	attributes := w.Attributes
	process := Process{
		ProcedureID:   int64(attributes.ProcedureID),
		Number:        strings.TrimSpace(attributes.Number),
		Type:          html.UnescapeString(strings.TrimSpace(attributes.Type)),
		Description:   html.UnescapeString(strings.TrimSpace(attributes.Description)),
		Assignee:      parseAssignee(attributes.Assignee),
		ReturnOverdue: attributes.Status.ReturnOverdue == "S",
		Origin:        attributes.Status.Origin,
	}
	if process.ProcedureID == 0 {
		process.ProcedureID = int64(w.ID)
	}
	if attributes.Status.Return == "S" {
		process.ReturnDate = parseReturnDate(attributes.Status.ReturnData)
	}
	return process
}

func parseAssignee(raw json.RawMessage) *User {
	var wire struct {
		UserID  flexInt `json:"idUsuario"`
		Acronym string  `json:"sigla"`
		Name    string  `json:"nome"`
	}
	if isEmptyJSON(raw) || json.Unmarshal(raw, &wire) != nil {
		return nil
	}
	if wire.UserID == 0 && strings.TrimSpace(wire.Acronym) == "" {
		return nil
	}
	return &User{ID: int64(wire.UserID), Acronym: strings.TrimSpace(wire.Acronym), Name: html.UnescapeString(strings.TrimSpace(wire.Name))}
}

func parseReturnDate(raw json.RawMessage) time.Time {
	var object struct {
		Date string `json:"dataProgramada"`
	}
	if json.Unmarshal(raw, &object) == nil && object.Date != "" {
		return parseDate(object.Date)
	}
	var text string
	if json.Unmarshal(raw, &text) == nil {
		return parseDate(text)
	}
	return time.Time{}
}

func parseDate(text string) time.Time {
	date, err := time.Parse(dateLayout, strings.TrimSpace(text))
	if err != nil {
		return time.Time{}
	}
	return date
}

type activityWire struct {
	ID         flexInt `json:"id"`
	Attributes struct {
		ProcedureID flexInt `json:"idProcesso"`
		User        string  `json:"usuario"`
		Date        string  `json:"data"`
		Time        string  `json:"hora"`
		Unit        string  `json:"unidade"`
		Text        string  `json:"informacao"`
	} `json:"atributos"`
}

func (w activityWire) activity(procedureID int64, location *time.Location) Activity {
	attributes := w.Attributes
	activity := Activity{
		ID:          int64(w.ID),
		ProcedureID: int64(attributes.ProcedureID),
		User:        strings.TrimSpace(attributes.User),
		Unit:        strings.TrimSpace(attributes.Unit),
		Text:        html.UnescapeString(strings.TrimSpace(attributes.Text)),
	}
	if activity.ProcedureID == 0 {
		activity.ProcedureID = procedureID
	}
	at, err := time.ParseInLocation(dateTimeLayout, strings.TrimSpace(attributes.Date)+" "+strings.TrimSpace(attributes.Time), location)
	if err == nil {
		activity.At = at.UTC()
	}
	return activity
}

type documentWire struct {
	ID         flexInt `json:"id"`
	Attributes struct {
		ProcedureID flexInt `json:"idProcedimento"`
		Protocol    string  `json:"protocoloFormatado"`
		Number      string  `json:"titulo"`
		Type        string  `json:"tipo"`
		FileName    string  `json:"nome"`
		Label       string  `json:"nomeComposto"`
	} `json:"atributos"`
}

func (w documentWire) document(procedureID int64) Document {
	attributes := w.Attributes
	document := Document{
		ID:          int64(w.ID),
		ProcedureID: int64(attributes.ProcedureID),
		Protocol:    strings.TrimSpace(attributes.Protocol),
		Type:        html.UnescapeString(strings.TrimSpace(attributes.Type)),
		Name:        documentName(attributes.Label, attributes.Type, attributes.Number, attributes.FileName),
	}
	if document.ProcedureID == 0 {
		document.ProcedureID = procedureID
	}
	return document
}

func documentName(label, kind, number, fileName string) string {
	if name := strings.TrimSpace(label); name != "" {
		return html.UnescapeString(name)
	}
	composed := strings.TrimSpace(strings.TrimSpace(kind) + " " + strings.TrimSpace(number))
	if composed != "" {
		return html.UnescapeString(composed)
	}
	return html.UnescapeString(strings.TrimSpace(fileName))
}
