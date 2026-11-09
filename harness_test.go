package main

import (
	"io/fs"
	"maps"
	"os"
	"testing"

	"github.com/ItzTas/sei-monitor/internal/core"
	"github.com/ItzTas/sei-monitor/internal/core/coretest"
	"github.com/ItzTas/sei-monitor/internal/sei/seitest"
)

func TestMain(m *testing.M) {
	os.Exit(coretest.Run(m))
}

func envSource(env map[string]string) core.Source {
	return core.Source{
		Lookup: func(name string) (string, bool) {
			value, ok := env[name]
			return value, ok
		},
		ReadFile: func(string) ([]byte, error) { return nil, fs.ErrNotExist },
	}
}

func seiEnv(server *seitest.Server, overrides map[string]string) map[string]string {
	env := map[string]string{
		"SEI_URL":      server.URL(),
		"SEI_USERNAME": seitest.Username,
		"SEI_PASSWORD": seitest.Password,
		"SEI_ORG":      seitest.OrgAcronym,
		"SEI_TIMEZONE": seitest.TimeZone,
	}
	maps.Copy(env, overrides)
	return env
}
