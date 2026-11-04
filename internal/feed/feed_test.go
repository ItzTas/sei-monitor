package feed

import (
	"context"
	"encoding/xml"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/ItzTas/sei-monitor/internal/config"
	"github.com/ItzTas/sei-monitor/internal/event"
)

const token = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG"

var now = time.Date(2026, 11, 4, 18, 30, 0, 0, time.UTC)

type fakeSource struct {
	events []event.Event
	limit  int
	err    error
}

func (s *fakeSource) RecentEvents(_ context.Context, limit int) ([]event.Event, error) {
	s.limit = limit
	return s.events, s.err
}

func sampleEvents() []event.Event {
	return []event.Event{
		{
			ID: 7, Kind: event.ActivityAdded,
			OccurredAt: time.Date(2026, 11, 4, 17, 13, 5, 0, time.UTC), DetectedAt: time.Date(2026, 11, 4, 17, 15, 0, 0, time.UTC),
			Payload: event.Payload{ProcessNumber: "00000.000001/2026-00", Unit: "SEDEMO/NUPROC", ActivityText: `<script>alert("x")</script> recebido`},
		},
		{
			ID: 6, Kind: event.ProcessEntered,
			OccurredAt: time.Date(2026, 11, 4, 17, 0, 0, 0, time.UTC), DetectedAt: time.Date(2026, 11, 4, 17, 0, 0, 0, time.UTC),
			Payload: event.Payload{ProcessNumber: "00000.000002/2026-00", Unit: "SEDEMO/NUPROC", Description: "Compra & venda"},
		},
	}
}

type parsed struct {
	Version string `xml:"version,attr"`
	Channel struct {
		Title       string `xml:"title"`
		Link        string `xml:"link"`
		Description string `xml:"description"`
		Items       []struct {
			Title string `xml:"title"`
			GUID  struct {
				Value       string `xml:",chardata"`
				IsPermaLink string `xml:"isPermaLink,attr"`
			} `xml:"guid"`
			PubDate     string `xml:"pubDate"`
			Description string `xml:"description"`
		} `xml:"item"`
	} `xml:"channel"`
}

func render(t *testing.T, full bool) parsed {
	t.Helper()
	var out strings.Builder
	formatter := event.Formatter{Location: time.UTC, Full: full}
	if err := Write(&out, Channel{Title: "sei-monitor: SEDEMO", Link: "https://sei.example.org"}, sampleEvents(), formatter, now); err != nil {
		t.Fatal(err)
	}
	var doc parsed
	if err := xml.Unmarshal([]byte(out.String()), &doc); err != nil {
		t.Fatalf("the feed is not valid XML: %v\n%s", err, out.String())
	}
	return doc
}

func TestWriteProducesAnRSS2Feed(t *testing.T) {
	doc := render(t, false)

	if doc.Version != "2.0" || doc.Channel.Title != "sei-monitor: SEDEMO" || doc.Channel.Link != "https://sei.example.org" || doc.Channel.Description == "" {
		t.Fatalf("channel = %+v", doc.Channel)
	}
	if len(doc.Channel.Items) != 2 {
		t.Fatalf("items = %d, want 2", len(doc.Channel.Items))
	}
	first := doc.Channel.Items[0]
	if first.Title != "New activity on process 00000.000001/2026-00 (2026-11-04 17:13)" {
		t.Errorf("title = %q", first.Title)
	}
	if first.GUID.Value != "7-20261104T171500Z" || first.GUID.IsPermaLink != "false" {
		t.Errorf("guid = %+v", first.GUID)
	}
	if pub, err := time.Parse(time.RFC1123Z, first.PubDate); err != nil || !pub.Equal(sampleEvents()[0].OccurredAt) {
		t.Errorf("pubDate = %q, %v", first.PubDate, err)
	}
	if first.Description != "When: 2026-11-04 17:13<br>Unit: SEDEMO/NUPROC" {
		t.Errorf("minimal description = %q", first.Description)
	}
}

func TestFullDescriptionEscapesSEIContent(t *testing.T) {
	doc := render(t, true)

	description := doc.Channel.Items[0].Description
	if strings.Contains(description, "<script>") || !strings.Contains(description, "&lt;script&gt;alert(&#34;x&#34;)&lt;/script&gt; recebido") {
		t.Errorf("description = %q, want the SEI text escaped as HTML", description)
	}
	if !strings.Contains(doc.Channel.Items[1].Description, "Description: Compra &amp; venda") {
		t.Errorf("description = %q", doc.Channel.Items[1].Description)
	}
}

func newHandler(source Source) *Handler {
	channel := func(context.Context) Channel {
		return Channel{Title: "sei-monitor: SEDEMO", Link: "https://sei.example.org"}
	}
	return NewHandler(source, config.NewSecret(token), channel, event.Formatter{}, func() time.Time { return now }, slog.New(slog.DiscardHandler))
}

func serve(t *testing.T, handler http.Handler, path string) *httptest.ResponseRecorder {
	t.Helper()
	mux := http.NewServeMux()
	mux.Handle("GET /feed/{file}", handler)
	recorder := httptest.NewRecorder()
	mux.ServeHTTP(recorder, httptest.NewRequestWithContext(t.Context(), http.MethodGet, path, nil))
	return recorder
}

func TestHandlerServesTheFeedForTheRightToken(t *testing.T) {
	source := &fakeSource{events: sampleEvents()}

	response := serve(t, newHandler(source), "/feed/"+token+".xml")

	if response.Code != http.StatusOK || response.Header().Get("Content-Type") != "application/rss+xml; charset=utf-8" {
		t.Fatalf("response = %d %q", response.Code, response.Header().Get("Content-Type"))
	}
	if source.limit != MaxItems || !strings.Contains(response.Body.String(), "<rss version=\"2.0\">") {
		t.Errorf("limit = %d, body:\n%s", source.limit, response.Body.String())
	}
}

func TestHandlerHidesTheFeedBehindTheToken(t *testing.T) {
	for _, path := range []string{"/feed/" + token, "/feed/" + token[:len(token)-1] + ".xml", "/feed/wrong.xml", "/feed/" + token + ".xml.bak"} {
		response := serve(t, newHandler(&fakeSource{events: sampleEvents()}), path)
		body, _ := io.ReadAll(response.Body)
		if response.Code != http.StatusNotFound || strings.Contains(string(body), "sei-monitor") {
			t.Errorf("%s: response = %d %q, want a plain 404", path, response.Code, body)
		}
	}
}

func TestHandlerKeepsStorageErrorsOutOfTheResponse(t *testing.T) {
	response := serve(t, newHandler(&fakeSource{err: errors.New("store: read the events: disk I/O error at /data/sei-monitor.db")}), "/feed/"+token+".xml")

	if response.Code != http.StatusInternalServerError || strings.Contains(response.Body.String(), "/data") {
		t.Errorf("response = %d %q", response.Code, response.Body.String())
	}
}

func TestSelfLinkUsesTheAtomNamespace(t *testing.T) {
	var out strings.Builder
	channel := Channel{Title: "sei-monitor: SEDEMO", Link: "https://sei.example.org", Self: "https://monitor.example.org/api/v1/feed/" + token + ".xml"}
	if err := Write(&out, channel, sampleEvents(), event.Formatter{}, now); err != nil {
		t.Fatal(err)
	}

	var doc struct {
		Channel struct {
			Self struct {
				Href string `xml:"href,attr"`
				Rel  string `xml:"rel,attr"`
				Type string `xml:"type,attr"`
			} `xml:"http://www.w3.org/2005/Atom link"`
		} `xml:"channel"`
	}
	if err := xml.Unmarshal([]byte(out.String()), &doc); err != nil {
		t.Fatal(err)
	}
	if doc.Channel.Self.Href != channel.Self || doc.Channel.Self.Rel != "self" || doc.Channel.Self.Type != "application/rss+xml" {
		t.Errorf("atom:link = %+v\n%s", doc.Channel.Self, out.String())
	}
}

func TestFeedWithoutSelfLinkLeavesAtomAndTheTokenOut(t *testing.T) {
	response := serve(t, newHandler(&fakeSource{events: sampleEvents()}), "/feed/"+token+".xml")

	if body := response.Body.String(); strings.Contains(body, "atom") || strings.Contains(body, token) {
		t.Errorf("feed without a self link:\n%s", body)
	}
}
