package store

import (
	"context"
	"database/sql/driver"
	"errors"
	"strconv"
	"strings"

	"github.com/jackc/pgx/v5/stdlib"
)

type rebindConnector struct {
	driver.Connector
}

func (c rebindConnector) Connect(ctx context.Context) (driver.Conn, error) {
	conn, err := c.Connector.Connect(ctx)
	if err != nil {
		return nil, err
	}
	pgConn, ok := conn.(*stdlib.Conn)
	if !ok {
		return nil, errors.Join(errors.New("store: unexpected Postgres connection type"), conn.Close())
	}
	return &rebindConn{pgConn}, nil
}

type rebindConn struct {
	*stdlib.Conn
}

func (c *rebindConn) Prepare(query string) (driver.Stmt, error) {
	return c.Conn.Prepare(rebind(query))
}

func (c *rebindConn) PrepareContext(ctx context.Context, query string) (driver.Stmt, error) {
	return c.Conn.PrepareContext(ctx, rebind(query))
}

func (c *rebindConn) ExecContext(ctx context.Context, query string, args []driver.NamedValue) (driver.Result, error) {
	return c.Conn.ExecContext(ctx, rebind(query), args)
}

func (c *rebindConn) QueryContext(ctx context.Context, query string, args []driver.NamedValue) (driver.Rows, error) {
	return c.Conn.QueryContext(ctx, rebind(query), args)
}

func rebind(query string) string {
	if !strings.Contains(query, "?") {
		return query
	}
	var b strings.Builder
	b.Grow(len(query) + 16)
	placeholder := 0
	for i := 0; i < len(query); {
		if skip := literalLength(query[i:]); skip > 0 {
			b.WriteString(query[i : i+skip])
			i += skip
			continue
		}
		if query[i] != '?' {
			b.WriteByte(query[i])
			i++
			continue
		}
		placeholder++
		b.WriteByte('$')
		b.WriteString(strconv.Itoa(placeholder))
		i++
	}
	return b.String()
}

func literalLength(rest string) int {
	delimiters := []struct{ open, close string }{{"'", "'"}, {`"`, `"`}, {"--", "\n"}, {"/*", "*/"}}
	for _, delimiter := range delimiters {
		if !strings.HasPrefix(rest, delimiter.open) {
			continue
		}
		end := strings.Index(rest[len(delimiter.open):], delimiter.close)
		if end < 0 {
			return len(rest)
		}
		return len(delimiter.open) + end + len(delimiter.close)
	}
	return 0
}
