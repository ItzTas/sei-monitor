package sei

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"

	"golang.org/x/net/html"
	"golang.org/x/net/html/atom"
	"golang.org/x/net/html/charset"
)

const orgSelect = "selOrgao"

func ListOrgs(ctx context.Context, seiURL string, opts Options) ([]Org, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, seiURL+"/sei/", nil)
	if err != nil {
		return nil, fmt.Errorf("sei: build the login page request: %w", withoutQuery(err))
	}
	req.Header.Set("User-Agent", opts.UserAgent)
	resp, err := opts.pageClient().Do(req)
	if err != nil {
		return nil, fmt.Errorf("sei: open the login page: %w", withoutQuery(err))
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("sei: the login page answered HTTP %d", resp.StatusCode)
	}

	reader, err := charset.NewReader(io.LimitReader(resp.Body, maxBodySize), resp.Header.Get("Content-Type"))
	if err != nil {
		return nil, fmt.Errorf("sei: decode the login page: %w", err)
	}
	doc, err := html.Parse(reader)
	if err != nil {
		return nil, fmt.Errorf("sei: read the login page: %w", err)
	}
	orgs := orgOptions(doc)
	if len(orgs) == 0 {
		return nil, errors.New("sei: the login page has no agency list (selOrgao field)")
	}
	return orgs, nil
}

func ResolveOrg(ctx context.Context, seiURL, value string, opts Options) (Org, error) {
	value = strings.TrimSpace(value)
	if id, err := strconv.ParseInt(value, 10, 64); err == nil && id > 0 {
		return Org{ID: id}, nil
	}
	orgs, err := ListOrgs(ctx, seiURL, opts)
	if err != nil {
		return Org{}, err
	}
	for _, org := range orgs {
		if strings.EqualFold(org.Acronym, value) {
			return org, nil
		}
	}
	return Org{}, fmt.Errorf("%w: %q (the orgs subcommand lists the available acronyms)", ErrOrgNotFound, value)
}

func orgOptions(doc *html.Node) []Org {
	for node := range doc.Descendants() {
		if node.Type == html.ElementNode && node.DataAtom == atom.Select && (attribute(node, "id") == orgSelect || attribute(node, "name") == orgSelect) {
			return options(node)
		}
	}
	return nil
}

func options(selectNode *html.Node) []Org {
	var orgs []Org
	for node := range selectNode.Descendants() {
		if node.Type != html.ElementNode || node.DataAtom != atom.Option {
			continue
		}
		id, err := strconv.ParseInt(strings.TrimSpace(attribute(node, "value")), 10, 64)
		if err != nil || id <= 0 {
			continue
		}
		orgs = append(orgs, Org{ID: id, Acronym: textContent(node)})
	}
	return orgs
}

func attribute(node *html.Node, name string) string {
	for _, attr := range node.Attr {
		if attr.Key == name {
			return attr.Val
		}
	}
	return ""
}

func textContent(node *html.Node) string {
	var builder strings.Builder
	for child := range node.Descendants() {
		if child.Type == html.TextNode {
			builder.WriteString(child.Data)
		}
	}
	return strings.Join(strings.Fields(builder.String()), " ")
}
