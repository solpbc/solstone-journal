// classify: reads file paths from args, prints JSON lines with licenseclassifier/v2 matches.
package main

import (
	"encoding/json"
	"fmt"
	"os"

	"github.com/google/licenseclassifier/v2/assets"
)

type m struct {
	Name       string  `json:"name"`
	MatchType  string  `json:"type"`
	Confidence float64 `json:"conf"`
	StartLine  int     `json:"start"`
	EndLine    int     `json:"end"`
}
type r struct {
	File    string `json:"file"`
	Lines   int    `json:"lines"`
	Matches []m    `json:"matches"`
}

func main() {
	c, err := assets.DefaultClassifier()
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	enc := json.NewEncoder(os.Stdout)
	for _, f := range os.Args[1:] {
		b, err := os.ReadFile(f)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(2)
		}
		n := 1
		for _, ch := range b {
			if ch == '\n' {
				n++
			}
		}
		res := c.Match(b)
		out := r{File: f, Lines: n}
		for _, x := range res.Matches {
			out.Matches = append(out.Matches, m{x.Name, x.MatchType, x.Confidence, x.StartLine, x.EndLine})
		}
		enc.Encode(out)
	}
}
