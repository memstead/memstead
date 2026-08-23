# Flags on other commands are out of scope

```bash
git log --oneline --bogus-flag
curl --fail-with-body https://example.test | memstead health --strict
```
<!-- expect: 0 -->
