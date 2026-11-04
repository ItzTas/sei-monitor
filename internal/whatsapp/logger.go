package whatsapp

import (
	"fmt"
	"log/slog"

	waLog "go.mau.fi/whatsmeow/util/log"
)

type logger struct {
	log    *slog.Logger
	module string
}

func newLogger(log *slog.Logger, module string) waLog.Logger {
	return logger{log: log, module: module}
}

func (l logger) Errorf(msg string, args ...any) {
	l.log.Error(fmt.Sprintf(msg, args...), "module", l.module)
}

func (l logger) Warnf(msg string, args ...any) {
	l.log.Warn(fmt.Sprintf(msg, args...), "module", l.module)
}

func (l logger) Infof(msg string, args ...any) {
	l.log.Debug(fmt.Sprintf(msg, args...), "module", l.module)
}

func (l logger) Debugf(string, ...any) {}

func (l logger) Sub(module string) waLog.Logger {
	return logger{log: l.log, module: l.module + "/" + module}
}
