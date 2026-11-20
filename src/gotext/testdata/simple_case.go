//go:build ignore

package main

import (
	"bufio"
	"fmt"
	"os"
	"runtime"
	"unicode"
)

func main() {
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	fmt.Fprintf(out, "%s unicode %s\n", runtime.Version(), unicode.Version)
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if r >= 0xD800 && r <= 0xDFFF {
			continue
		}
		lower, upper, fold := unicode.ToLower(r), unicode.ToUpper(r), unicode.SimpleFold(r)
		if lower != r || upper != r || fold != r {
			fmt.Fprintf(out, "%X %X %X %X\n", r, lower, upper, fold)
		}
	}
}
