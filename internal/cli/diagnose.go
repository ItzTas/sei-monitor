package cli

import (
	"bytes"
	"cmp"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"net/url"
	"slices"
	"strconv"
	"strings"
	"unicode"

	"github.com/spf13/cobra"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/sei"
)

const (
	pathProcesses  = "/processo/listar"
	pathActivities = "/atividade/listar"
	pathDocuments  = "/documento/listar/"
)

type item map[string]json.RawMessage

type listing struct {
	items   []item
	total   json.RawMessage
	hasData bool
}

type diagnosis struct {
	client *sei.Client
	unit   int64
	out    *printer
}

func (a *app) diagnose(cmd *cobra.Command, _ []string) error {
	cfg, err := core.LoadSEI(a.source)
	if err != nil {
		return err
	}
	ctx := cmd.Context()
	conn, err := connect(ctx, cfg)
	if err != nil {
		return err
	}
	session, err := conn.Client.Login(ctx)
	if err != nil {
		return err
	}
	watched, err := session.SelectUnits(cfg.Units, cfg.AllUnits)
	if err != nil {
		return fmt.Errorf("SEI_UNITS: %w", err)
	}
	version, err := conn.Client.Version(ctx)
	if err != nil {
		return err
	}

	d := &diagnosis{client: conn.Client, unit: watched[0].ID, out: &printer{w: cmd.OutOrStdout()}}
	d.out.printf("sei-monitor diagnose\n")
	d.out.printf("SEI %s, mod-wssei %s; units: %d available, %d watched (checks use the first one)\n", version.SEI, version.Module, len(session.Units), len(watched))
	d.run(ctx)
	return d.out.err
}

func (d *diagnosis) run(ctx context.Context) {
	processes, err := d.list(ctx, pathProcesses, url.Values{"limit": {"100"}, "start": {"0"}})
	if err != nil {
		d.out.printf("\ncould not list the processes: %s\n", describeFailure(err))
		return
	}
	procedure, hasProcedure := scalar(field(firstItem(processes.items), "atributos", "idProcedimento"))
	activityPath, documentPath := "", ""
	if hasProcedure {
		activityPath = pathActivities + "?procedimento=" + url.QueryEscape(procedure)
		documentPath = pathDocuments + url.PathEscape(procedure)
	}

	d.section("[1] Pagination with limit=2")
	d.pagination(ctx, pathProcesses, pathProcesses)
	d.pagination(ctx, pathActivities, activityPath)
	d.pagination(ctx, "/documento/listar", documentPath)

	d.section("[2] Coverage of /processo/listar by processoGeradoRecebido")
	for _, kind := range []string{"", "R", "G"} {
		d.coverage(ctx, kind)
	}

	d.section("[3] Scheduled returns (first 100 processes)")
	d.returns(processes.items)

	d.section("[4] Fields that may hold a link")
	d.links(processes.items)

	d.section("[5] Field types")
	d.types(ctx, processes, activityPath, documentPath)

	d.section("[6] Order")
	d.order(ctx, "activities", activityPath)
	d.order(ctx, "documents", documentPath)
}

func (d *diagnosis) section(title string) {
	d.out.printf("\n%s\n", title)
}

func (d *diagnosis) line(format string, args ...any) {
	d.out.printf("    "+format+"\n", args...)
}

func (d *diagnosis) list(ctx context.Context, path string, query url.Values) (listing, error) {
	base, rawQuery, _ := strings.Cut(path, "?")
	values, err := url.ParseQuery(rawQuery)
	if err != nil {
		return listing{}, err
	}
	maps.Copy(values, query)
	page, err := d.client.Raw(ctx, d.unit, base, values)
	if err != nil {
		return listing{}, err
	}
	result := listing{total: page.Total, hasData: page.Data != nil}
	if kind(page.Data) == "list" {
		_ = json.Unmarshal(page.Data, &result.items)
	}
	return result, nil
}

func (d *diagnosis) pagination(ctx context.Context, name, path string) {
	if path == "" {
		d.line("%s: no process to check with", name)
		return
	}
	first, errFirst := d.list(ctx, path, url.Values{"limit": {"2"}, "start": {"0"}})
	second, errSecond := d.list(ctx, path, url.Values{"limit": {"2"}, "start": {"1"}})
	if err := errors.Join(errFirst, errSecond); err != nil {
		d.line("%s: %s", name, describeFailure(err))
		return
	}
	firstIDs, secondIDs := ids(first.items), ids(second.items)
	shared := 0
	for _, id := range secondIDs {
		if slices.Contains(firstIDs, id) {
			shared++
		}
	}
	verdict := "start is a page number"
	if len(firstIDs) < 2 || len(secondIDs) == 0 {
		verdict = "not enough items to tell"
	} else if shared > 0 {
		verdict = "start looks like an offset"
	}
	d.line("%s: page 0 has %d ids, page 1 has %d, %d in common -> %s", name, len(firstIDs), len(secondIDs), shared, verdict)
}

func (d *diagnosis) coverage(ctx context.Context, kind string) {
	query := url.Values{"limit": {"100"}, "start": {"0"}}
	label := "default call"
	if kind != "" {
		query.Set("tipo", kind)
		label = "tipo=" + kind
	}
	result, err := d.list(ctx, pathProcesses, query)
	if err != nil {
		d.line("%s: %s", label, describeFailure(err))
		return
	}
	counts := map[string]int{"G": 0, "R": 0, "other": 0}
	for _, it := range result.items {
		value, _ := scalar(field(it, "atributos", "status", "processoGeradoRecebido"))
		if _, known := counts[value]; !known || value == "other" {
			value = "other"
		}
		counts[value]++
	}
	d.line("%-13s total %s; G=%d R=%d other=%d", label+":", count(result.total), counts["G"], counts["R"], counts["other"])
}

func (d *diagnosis) returns(processes []item) {
	scheduled, carried := 0, 0
	kinds, shapes := map[string]int{}, map[string]int{}
	for _, it := range processes {
		flag, _ := scalar(field(it, "atributos", "status", "retornoProgramado"))
		data := field(it, "atributos", "status", "retornoData")
		if flag != "S" {
			if kind(data) != "null" && kind(data) != "missing" {
				carried++
			}
			continue
		}
		scheduled++
		kinds[describeKind(data)]++
		if date, ok := scalar(field(item{"x": data}, "x", "dataProgramada")); ok {
			shapes[shape(date)]++
		}
	}
	d.line("retornoProgramado=S: %d; retornoData: %s", scheduled, tally(kinds))
	d.line("dataProgramada: %s", tally(shapes))
	d.line("retornoProgramado=N with retornoData filled: %d (sei-monitor ignores those)", carried)
}

func (d *diagnosis) links(processes []item) {
	found := map[string]bool{}
	for _, it := range processes {
		collectLinkKeys("", it, found)
	}
	if len(found) == 0 {
		d.line("none in the process items")
	} else {
		d.line("%s", strings.Join(slices.Sorted(maps.Keys(found)), ", "))
	}
	d.line("a stable link to a process still needs a manual check in the browser")
}

func (d *diagnosis) types(ctx context.Context, processes listing, activityPath, documentPath string) {
	d.line("process: id %s; idProcedimento %s; usuarioAtribuido %s; total %s", fieldKinds(processes.items, "id"), fieldKinds(processes.items, "atributos", "idProcedimento"), fieldKinds(processes.items, "atributos", "usuarioAtribuido"), kind(processes.total))
	if activityPath != "" {
		if activities, err := d.list(ctx, activityPath, url.Values{"limit": {"10"}, "start": {"0"}}); err == nil {
			d.line("activity: id %s; data %s; hora %s; usuario %s", fieldKinds(activities.items, "id"), fieldKinds(activities.items, "atributos", "data"), fieldKinds(activities.items, "atributos", "hora"), fieldKinds(activities.items, "atributos", "usuario"))
		}
	}
	if documentPath != "" {
		if documents, err := d.list(ctx, documentPath, url.Values{"limit": {"10"}, "start": {"0"}}); err == nil {
			d.line("document: id %s; nomeComposto %s; protocoloFormatado %s", fieldKinds(documents.items, "id"), fieldKinds(documents.items, "atributos", "nomeComposto"), fieldKinds(documents.items, "atributos", "protocoloFormatado"))
		}
	}
	empty, err := d.list(ctx, pathProcesses, url.Values{"limit": {"1"}, "start": {"100000"}})
	if err != nil {
		d.line("empty page: %s", describeFailure(err))
		return
	}
	d.line("empty page: data key %s, total %s", map[bool]string{true: "present", false: "absent"}[empty.hasData], kind(empty.total))
}

func (d *diagnosis) order(ctx context.Context, name, path string) {
	if path == "" {
		d.line("%s: no process to check with", name)
		return
	}
	result, err := d.list(ctx, path, url.Values{"limit": {"10"}, "start": {"0"}})
	if err != nil {
		d.line("%s: %s", name, describeFailure(err))
		return
	}
	d.line("%s: %s", name, sequence(ids(result.items)))
}

func describeFailure(err error) string {
	reasons := []struct {
		target error
		text   string
	}{
		{context.DeadlineExceeded, "the call timed out"},
		{sei.ErrUnauthorized, "SEI rejected the session token"},
		{sei.ErrServer, "SEI answered with an internal error"},
		{sei.ErrUnexpectedResponse, "SEI answered outside the API format"},
		{sei.ErrRedirect, "SEI answered with a redirect"},
	}
	for _, reason := range reasons {
		if errors.Is(err, reason.target) {
			return reason.text
		}
	}
	if apiErr, ok := errors.AsType[*sei.APIError](err); ok {
		return fmt.Sprintf("SEI refused the call (HTTP %d)", apiErr.Status)
	}
	if _, ok := errors.AsType[*url.Error](err); ok {
		return "network error"
	}
	return "the call failed"
}

func firstItem(items []item) item {
	if len(items) == 0 {
		return nil
	}
	return items[0]
}

func field(it item, path ...string) json.RawMessage {
	current := it
	for i, key := range path {
		value, ok := current[key]
		if !ok {
			return nil
		}
		if i == len(path)-1 {
			return value
		}
		var next item
		if json.Unmarshal(value, &next) != nil {
			return nil
		}
		current = next
	}
	return nil
}

func kind(raw json.RawMessage) string {
	trimmed := bytes.TrimSpace(raw)
	if len(trimmed) == 0 {
		return "missing"
	}
	kinds := map[byte]string{'"': "string", '{': "object", '[': "list", 't': "boolean", 'f': "boolean", 'n': "null"}
	if name, ok := kinds[trimmed[0]]; ok {
		return name
	}
	return "number"
}

func describeKind(raw json.RawMessage) string {
	var object item
	if kind(raw) != "object" || json.Unmarshal(raw, &object) != nil {
		return kind(raw)
	}
	return "object{" + strings.Join(slices.Sorted(maps.Keys(object)), ",") + "}"
}

func scalar(raw json.RawMessage) (string, bool) {
	var text string
	if json.Unmarshal(raw, &text) == nil {
		return text, true
	}
	var number json.Number
	if json.Unmarshal(raw, &number) == nil {
		return number.String(), true
	}
	return "", false
}

func ids(items []item) []string {
	result := make([]string, 0, len(items))
	for _, it := range items {
		if id, ok := scalar(it["id"]); ok {
			result = append(result, id)
		}
	}
	return result
}

func fieldKinds(items []item, path ...string) string {
	kinds := map[string]int{}
	for _, it := range items {
		kinds[kind(field(it, path...))]++
	}
	return tally(kinds)
}

func tally(counts map[string]int) string {
	if len(counts) == 0 {
		return "none"
	}
	parts := make([]string, 0, len(counts))
	for _, name := range slices.Sorted(maps.Keys(counts)) {
		parts = append(parts, fmt.Sprintf("%s x%d", name, counts[name]))
	}
	return strings.Join(parts, ", ")
}

func shape(text string) string {
	runes := []rune(text)
	if len(runes) > 24 {
		runes = runes[:24]
	}
	for i, r := range runes {
		if unicode.IsDigit(r) {
			runes[i] = '9'
		} else if unicode.IsLetter(r) {
			runes[i] = 'A'
		}
	}
	return string(runes)
}

func count(raw json.RawMessage) string {
	text, ok := scalar(raw)
	if _, err := strconv.Atoi(text); !ok || err != nil {
		return kind(raw)
	}
	return text
}

func sequence(values []string) string {
	numbers := make([]int64, 0, len(values))
	for _, value := range values {
		n, err := strconv.ParseInt(value, 10, 64)
		if err != nil {
			return "ids are not numbers"
		}
		numbers = append(numbers, n)
	}
	if len(numbers) < 2 {
		return "too few items to tell"
	}
	descending := slices.IsSortedFunc(numbers, func(a, b int64) int { return cmp.Compare(b, a) })
	ascending := slices.IsSorted(numbers)
	verdicts := map[[2]bool]string{
		{true, false}: "newest first (ids going down)",
		{false, true}: "oldest first (ids going up)",
	}
	if verdict, ok := verdicts[[2]bool{descending, ascending}]; ok {
		return verdict
	}
	return "mixed order"
}

func collectLinkKeys(prefix string, it item, found map[string]bool) {
	for key, value := range it {
		path := strings.TrimPrefix(prefix+"."+shape(key), ".")
		lower := strings.ToLower(key)
		if strings.Contains(lower, "link") || strings.Contains(lower, "url") || strings.Contains(lower, "hash") || strings.Contains(lower, "href") {
			found[path] = true
		}
		var nested item
		if kind(value) == "object" && json.Unmarshal(value, &nested) == nil {
			collectLinkKeys(path, nested, found)
		}
	}
}
