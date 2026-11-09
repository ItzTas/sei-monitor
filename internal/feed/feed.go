package feed

import (
	"bytes"
	"context"
	"crypto/subtle"
	"encoding/xml"
	"fmt"
	"html"
	"io"
	"log/slog"
	"net/http"
	"strings"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/event"
)

const MaxItems = 100

type Source interface {
	RecentEvents(ctx context.Context, limit int) ([]event.Event, error)
}

type Channel struct {
	Title string
	Link  string
	Self  string
}

type rss struct {
	XMLName xml.Name   `xml:"rss"`
	Version string     `xml:"version,attr"`
	Atom    string     `xml:"xmlns:atom,attr,omitempty"`
	Channel rssChannel `xml:"channel"`
}

type rssChannel struct {
	Title         string    `xml:"title"`
	Link          string    `xml:"link"`
	Self          *atomLink `xml:"atom:link,omitempty"`
	Description   string    `xml:"description"`
	LastBuildDate string    `xml:"lastBuildDate"`
	Generator     string    `xml:"generator"`
	Items         []rssItem `xml:"item"`
}

type atomLink struct {
	Href string `xml:"href,attr"`
	Rel  string `xml:"rel,attr"`
	Type string `xml:"type,attr"`
}

type rssItem struct {
	Title       string  `xml:"title"`
	Link        string  `xml:"link,omitempty"`
	GUID        rssGUID `xml:"guid"`
	PubDate     string  `xml:"pubDate"`
	Description string  `xml:"description"`
}

type rssGUID struct {
	Value       string `xml:",chardata"`
	IsPermaLink string `xml:"isPermaLink,attr"`
}

func Write(w io.Writer, channel Channel, events []event.Event, formatter event.Formatter, now time.Time) error {
	doc := rss{
		Version: "2.0",
		Channel: rssChannel{
			Title:         channel.Title,
			Link:          channel.Link,
			Description:   "Changes in your SEI processes",
			LastBuildDate: now.UTC().Format(time.RFC1123Z),
			Generator:     "sei-monitor",
		},
	}
	if channel.Self != "" {
		doc.Atom = "http://www.w3.org/2005/Atom"
		doc.Channel.Self = &atomLink{Href: channel.Self, Rel: "self", Type: "application/rss+xml"}
	}
	for _, ev := range events {
		doc.Channel.Items = append(doc.Channel.Items, item(ev, formatter, channel.Link))
	}
	if _, err := io.WriteString(w, xml.Header); err != nil {
		return err
	}
	encoder := xml.NewEncoder(w)
	encoder.Indent("", "  ")
	if err := encoder.Encode(doc); err != nil {
		return fmt.Errorf("feed: write the RSS document: %w", err)
	}
	return encoder.Close()
}

func item(ev event.Event, formatter event.Formatter, seiURL string) rssItem {
	lines := formatter.Details(ev)
	for i, line := range lines {
		lines[i] = html.EscapeString(line)
	}
	return rssItem{
		Title:       formatter.Title(ev),
		Link:        processLink(seiURL, ev.ProcedureID),
		GUID:        rssGUID{Value: fmt.Sprintf("%d-%s", ev.ID, ev.DetectedAt.UTC().Format("20060102T150405Z")), IsPermaLink: "false"},
		PubDate:     ev.OccurredAt.UTC().Format(time.RFC1123Z),
		Description: strings.Join(lines, "<br>"),
	}
}

func processLink(seiURL string, procedureID int64) string {
	if procedureID == 0 {
		return ""
	}
	return fmt.Sprintf("%s/sei/controlador.php?acao=procedimento_trabalhar&id_procedimento=%d", seiURL, procedureID)
}

type Handler struct {
	source    Source
	token     core.Secret
	channel   func(context.Context) Channel
	formatter func() event.Formatter
	now       func() time.Time
	log       *slog.Logger
}

func NewHandler(source Source, token core.Secret, channel func(context.Context) Channel, formatter func() event.Formatter, now func() time.Time, log *slog.Logger) *Handler {
	return &Handler{source: source, token: token, channel: channel, formatter: formatter, now: now, log: log}
}

func (h *Handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if !h.authorized(r.PathValue("file")) {
		http.NotFound(w, r)
		return
	}
	events, err := h.source.RecentEvents(r.Context(), MaxItems)
	if err != nil {
		h.log.Error("could not read the events for the feed", "error", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}
	var body bytes.Buffer
	if err := Write(&body, h.channel(r.Context()), events, h.formatter(), h.now()); err != nil {
		h.log.Error("could not build the feed", "error", err)
		http.Error(w, "internal error", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/rss+xml; charset=utf-8")
	_, _ = w.Write(body.Bytes())
}

func (h *Handler) authorized(file string) bool {
	return subtle.ConstantTimeCompare([]byte(file), []byte(h.token.Reveal()+".xml")) == 1
}
