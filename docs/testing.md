# Testing

## Test Structure

- **Framework**: pytest; coverage reporting comes from `make test-cov`, `make ci`, or `make coverage`, not bare `make test`
- **Unit Tests**: `tests/` root directory
  - Fast, no external API calls
  - Use `tests/fixtures/journal/` mock data
  - Test individual functions and modules
- **Integration Tests**: `tests/integration/` subdirectory
  - Test real backends (Anthropic, OpenAI, Google)
  - Require API keys in `.env`
  - Test end-to-end workflows
- **Naming**: Files `test_*.py`, functions `test_*`
- **Fixtures**: Shared fixtures in `tests/conftest.py`

## Fixture Journal

```python
# The autouse set_test_journal_path fixture in tests/conftest.py does this
# for unit tests. Set it explicitly only when a test needs a different journal.
os.environ["SOLSTONE_JOURNAL"] = "tests/fixtures/journal"
# Now all journal operations work with test data
```

The `tests/fixtures/journal/` directory contains a complete mock journal structure with sample facets, agents, transcripts, and indexed data for testing.

## Running Tests

- `make test` for unit tests
- `make test-cov` for unit tests with coverage reporting
- `make test-apps` to run app tests
- `make test-integration` for integration tests
- `make test-all` to run all tests (core + apps + integration)
- `make test-only TEST=path` to run specific tests
- `make coverage` to generate a coverage report
- `make ci` before committing (formats, lints, tests)
- Always run `sol restart-convey` after editing `solstone/convey/` or `solstone/apps/` to reload code

### Browser verification status

Browser verification is CDP-only in `tests/verify_browser.py`. Pinchtab is retained to launch Chrome and expose the debug port; the harness drives pages through CDP because CTO advisory `req_4kcmthzo` diagnosed Pinchtab REST instability, around 60% per-request rejection under long-lived state, as the driver of the old full-suite failures.

## Worktree Development

Run the full stack (supervisor + callosum + sense + cortex + convey) against test fixture data:

```bash
make dev                    # Start stack (Ctrl+C to stop)
```

In a second terminal, hit endpoints:

```bash
export SOLSTONE_JOURNAL=tests/fixtures/journal
export PATH=$(pwd)/.venv/bin:$PATH
curl -s http://localhost:$(cat tests/fixtures/journal/health/convey.port)/
```

Notes:

- Agents won't execute without API keys — this is expected in worktrees
- Output artifacts go in `scratch/` (git-ignored)
- Service logs: `tests/fixtures/journal/health/<service>.log`
- `make dev` writes runtime artifacts (stats cache, health logs, task logs) into the fixtures journal — these are covered by `tests/fixtures/journal/.gitignore` and should never be committed
