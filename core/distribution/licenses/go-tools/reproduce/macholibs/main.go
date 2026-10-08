package main

import (
	"debug/macho"
	"fmt"
	"os"
)

func main() {
	f, err := macho.Open(os.Args[1])
	if err != nil {
		panic(err)
	}
	libs, err := f.ImportedLibraries()
	if err != nil {
		panic(err)
	}
	for _, l := range libs {
		fmt.Println(l)
	}
}
