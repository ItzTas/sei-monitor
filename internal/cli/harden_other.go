//go:build !linux

package cli

func harden() error {
	return nil
}
