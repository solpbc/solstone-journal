# Operational-log slug fixtures

Each mapping is `original|fallback|expected_slug`. Originals include Unicode,
whitespace, punctuation, path separators, and empty-after-collapse cases.

```
Hello World|source|hello-world
Cortex|source|cortex
cortex|source|cortex
a/b|source|a-b
a\b|source|a-b
foo---bar|source|foo-bar
maintenance:<task>|source|maintenance-task
trailing. |source|trailing
...|source|source
---|run|run
***|source|source
é|source|e
é|source|source
İ|source|source
   tabs	and spaces|source|tabs-and-spaces
stream:zone|run|stream-zone
```
