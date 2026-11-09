package ui

import (
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/ItzTas/sei-monitor/internal/core"
)

const (
	sessionCookie = "__Secure-sei-monitor-session"
	sessionTTL    = 7 * 24 * time.Hour
	sessionLabel  = "sei-monitor web session "
)

type sessions struct {
	token core.Secret
	now   func() time.Time
}

func (s sessions) accepts(candidate string) bool {
	return !s.token.IsZero() && subtle.ConstantTimeCompare([]byte(candidate), []byte(s.token.Reveal())) == 1
}

func (s sessions) issue() *http.Cookie {
	expires := s.now().Add(sessionTTL).Unix()
	return cookie(strconv.FormatInt(expires, 10)+"."+s.sign(expires), int(sessionTTL.Seconds()))
}

func (s sessions) clear() *http.Cookie {
	return cookie("", -1)
}

func (s sessions) valid(r *http.Request) bool {
	session, err := r.Cookie(sessionCookie)
	if err != nil {
		return false
	}
	expiresText, signature, found := strings.Cut(session.Value, ".")
	if !found {
		return false
	}
	expires, err := strconv.ParseInt(expiresText, 10, 64)
	if err != nil || s.now().Unix() >= expires {
		return false
	}
	return hmac.Equal([]byte(signature), []byte(s.sign(expires)))
}

func (s sessions) sign(expires int64) string {
	mac := hmac.New(sha256.New, []byte(s.token.Reveal()))
	_, _ = mac.Write([]byte(sessionLabel + strconv.FormatInt(expires, 10)))
	return base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
}

func cookie(value string, maxAge int) *http.Cookie {
	return &http.Cookie{
		Name:     sessionCookie,
		Value:    value,
		MaxAge:   maxAge,
		HttpOnly: true,
		Secure:   true,
		SameSite: http.SameSiteLaxMode,
	}
}
