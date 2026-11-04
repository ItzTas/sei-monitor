package store

import (
	"context"
	"database/sql"
	"embed"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/stdlib"
	"github.com/pressly/goose/v3"
	"github.com/pressly/goose/v3/lock"
	_ "modernc.org/sqlite"
)

const (
	DialectSQLite   = "sqlite"
	DialectPostgres = "postgres"
)

//go:embed migrations
var migrations embed.FS

type Store struct {
	db      *sql.DB
	dialect string
}

func Open(ctx context.Context, databaseURL string) (*Store, error) {
	if path, ok := strings.CutPrefix(databaseURL, "sqlite://"); ok {
		return openSQLite(ctx, path)
	}
	if strings.HasPrefix(databaseURL, "postgres://") || strings.HasPrefix(databaseURL, "postgresql://") {
		return openPostgres(ctx, databaseURL)
	}
	return nil, errors.New("store: DATABASE_URL must start with sqlite:// or postgres://")
}

func Migrate(ctx context.Context, databaseURL string) error {
	s, err := Open(ctx, databaseURL)
	if err != nil {
		return err
	}
	return s.Close()
}

func (s *Store) Close() error {
	return s.db.Close()
}

func (s *Store) Dialect() string {
	return s.dialect
}

func openSQLite(ctx context.Context, path string) (*Store, error) {
	if path == "" || strings.ContainsAny(path, "?#") {
		return nil, errors.New("store: the SQLite path in DATABASE_URL must be a plain file path")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, fmt.Errorf("store: create the database folder: %w", err)
	}
	dsn := "file:" + path + "?_pragma=foreign_keys(1)&_pragma=journal_mode(WAL)&_pragma=busy_timeout(5000)&_txlock=immediate"
	db, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("store: open SQLite: %w", err)
	}
	db.SetMaxOpenConns(4)
	if err := migrate(ctx, db, goose.DialectSQLite3, DialectSQLite, nil); err != nil {
		return nil, errors.Join(err, db.Close())
	}
	return &Store{db: db, dialect: DialectSQLite}, nil
}

func openPostgres(ctx context.Context, databaseURL string) (*Store, error) {
	config, err := pgx.ParseConfig(databaseURL)
	if err != nil {
		return nil, errors.New("store: DATABASE_URL is not a valid Postgres address")
	}
	locker, err := lock.NewPostgresSessionLocker()
	if err != nil {
		return nil, fmt.Errorf("store: prepare the migration lock: %w", err)
	}
	migrator := sql.OpenDB(stdlib.GetConnector(*config))
	migrateErr := migrate(ctx, migrator, goose.DialectPostgres, DialectPostgres, locker)
	if err := errors.Join(migrateErr, migrator.Close()); err != nil {
		return nil, err
	}

	db := sql.OpenDB(rebindConnector{stdlib.GetConnector(*config)})
	db.SetMaxOpenConns(10)
	db.SetMaxIdleConns(2)
	db.SetConnMaxIdleTime(5 * time.Minute)
	return &Store{db: db, dialect: DialectPostgres}, nil
}

func migrate(ctx context.Context, db *sql.DB, dialect goose.Dialect, folder string, locker lock.SessionLocker) error {
	files, err := fs.Sub(migrations, "migrations/"+folder)
	if err != nil {
		return fmt.Errorf("store: read the embedded migrations: %w", err)
	}
	options := []goose.ProviderOption{goose.WithDisableGlobalRegistry(true)}
	if locker != nil {
		options = append(options, goose.WithSessionLocker(locker))
	}
	provider, err := goose.NewProvider(dialect, db, files, options...)
	if err != nil {
		return fmt.Errorf("store: prepare the migrations: %w", err)
	}
	if _, err := provider.Up(ctx); err != nil {
		return fmt.Errorf("store: run the migrations: %w", err)
	}
	return nil
}

func (s *Store) inTx(ctx context.Context, run func(*sql.Tx) error) (err error) {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("store: begin a transaction: %w", err)
	}
	defer func() {
		if err != nil {
			err = errors.Join(err, tx.Rollback())
		}
	}()
	if err := run(tx); err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("store: commit: %w", err)
	}
	return nil
}
