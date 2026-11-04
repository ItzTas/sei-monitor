package sei

import (
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"unicode"

	"golang.org/x/text/runes"
	"golang.org/x/text/transform"
	"golang.org/x/text/unicode/norm"
)

var (
	ErrInvalidCredentials = errors.New("SEI rejected the login")
	ErrUnauthorized       = errors.New("SEI rejected the session token")
	ErrModuleNotFound     = errors.New("the mod-wssei module was not found on this SEI")
	ErrModuleUnavailable  = errors.New("the mod-wssei module is disabled or does not support this SEI version")
	ErrServer             = errors.New("SEI answered with an internal error")
	ErrRedirect           = errors.New("SEI answered with a redirect")
	ErrUnexpectedResponse = errors.New("SEI answered outside the API format")
	ErrOrgNotFound        = errors.New("agency not found on the SEI login page")
	ErrRefused            = errors.New("SEI refused the request")
)

type APIError struct {
	Route   string
	Status  int
	Message string
	kind    error
}

func (e *APIError) Error() string {
	if e.Message == "" {
		return fmt.Sprintf("sei: %s answered HTTP %d", e.Route, e.Status)
	}
	return fmt.Sprintf("sei: %s: %s", e.Route, e.Message)
}

func (e *APIError) Is(target error) bool {
	return e.kind != nil && e.kind == target
}

func newAPIError(route string, status int, message string) *APIError {
	return &APIError{Route: route, Status: status, Message: message, kind: classify(route, status, message)}
}

func classify(route string, status int, message string) error {
	folded := fold(message)
	rules := []struct {
		matches bool
		kind    error
	}{
		{strings.HasPrefix(folded, "modulo inativo") || strings.HasPrefix(folded, "modulo incompativel"), ErrModuleUnavailable},
		{status >= http.StatusInternalServerError, ErrServer},
		{route == routeLogin, ErrInvalidCredentials},
		{status == http.StatusUnauthorized || status == http.StatusForbidden || strings.Contains(folded, "token"), ErrUnauthorized},
	}
	for _, rule := range rules {
		if rule.matches {
			return rule.kind
		}
	}
	return ErrRefused
}

func fold(text string) string {
	stripped, _, err := transform.String(transform.Chain(norm.NFD, runes.Remove(runes.In(unicode.Mn)), norm.NFC), text)
	if err != nil {
		stripped = text
	}
	return strings.ToLower(strings.TrimSpace(stripped))
}

func withoutQuery(err error) error {
	urlErr, ok := errors.AsType[*url.Error](err)
	if !ok {
		return err
	}
	return &url.Error{Op: urlErr.Op, URL: stripURL(urlErr.URL), Err: urlErr.Err}
}

func stripURL(raw string) string {
	parsed, err := url.Parse(raw)
	if err != nil {
		return "[address hidden]"
	}
	parsed.User = nil
	parsed.RawQuery = ""
	parsed.ForceQuery = false
	parsed.Fragment = ""
	return parsed.String()
}
