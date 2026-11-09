package core

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/jackc/pgx/v5/stdlib"

	"github.com/ItzTas/sei-monitor/sql/schema"
)

type Database struct {
	Pool *pgxpool.Pool
	SQL  *sql.DB
}

func OpenDatabase(ctx context.Context, databaseURL string) (*Database, error) {
	if err := schema.Migrate(ctx, databaseURL); err != nil {
		return nil, err
	}
	config, err := pgxpool.ParseConfig(databaseURL)
	if err != nil {
		return nil, errors.New("database: DATABASE_URL is not a valid Postgres address")
	}
	config.MaxConns = 10
	config.MaxConnIdleTime = 5 * time.Minute
	pool, err := pgxpool.NewWithConfig(ctx, config)
	if err != nil {
		return nil, fmt.Errorf("database: open the pool: %w", err)
	}
	if err := pool.Ping(ctx); err != nil {
		pool.Close()
		return nil, fmt.Errorf("database: reach Postgres: %w", err)
	}
	return &Database{Pool: pool, SQL: stdlib.OpenDBFromPool(pool)}, nil
}

func (d *Database) Close() error {
	err := d.SQL.Close()
	d.Pool.Close()
	return err
}

func InTx(ctx context.Context, pool *pgxpool.Pool, run func(pgx.Tx) error) error {
	return pgx.BeginFunc(ctx, pool, run)
}
