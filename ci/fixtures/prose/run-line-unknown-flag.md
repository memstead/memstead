# A run: line in fenced yaml with an unknown flag

```yaml
jobs:
  check:
    steps:
      - run: memstead health --strict --nope
      - name: block
        run: |
          memstead quickstart --agent claude-code --repo .
          memstead health --strict
```
<!-- expect: 1 -->
