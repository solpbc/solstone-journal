# Backlog

Tactical work items prioritized for implementation.

---

## Apps

- [ ] Add tabs or navigation mode to entities app all-facet view (reduce vertical scrolling)
- [ ] Audit apps for #fragment deep linking and improve coverage

## Agents

- [ ] Update supervisor/think interaction to use dynamic daily schedule from daily schedule agent output
- [ ] Create segment agent for voiceprint detection and updating via hooks
- [ ] Surface named hook outputs in agents app and journal talent CLI
- [ ] Make daily schedule agents idempotent with state tracking (show existing vs new segments)
- [ ] Add activities attach/update CLI tools for facet curation (like entity tools)

## Integrations

- [ ] Add OpenRouter provider support with observe integration for multimodal models
- [ ] Automated Fireflies importer
- [ ] Investigate Gemini stopping early in chats

## Infrastructure

- [ ] Health monitor and diagnostics agent (explore Claude Code SDK)

## Indexer

- [ ] Performance tune SQLite usage
- [ ] Refactor entity detection to be per-entity

## Testing

- [x] Move fixtures/ into tests/
- [ ] Enable clean fixture-based service startup (Convey on dynamic port) for integration/dev testing
