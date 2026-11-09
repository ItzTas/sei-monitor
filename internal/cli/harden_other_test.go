//go:build !linux

package cli

func reportHardening() int {
	return 0
}
